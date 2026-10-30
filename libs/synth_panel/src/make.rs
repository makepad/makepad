//! A score's editor panel as its agent makes it (the shared driver,
//! [`makepad_editor_panel::make`], with what a score adds): the fixed
//! skeleton (the voice tree, the selected voice's timeline and variants),
//! and each instrument's tweaks written by writers at once, an instrument's
//! live paths (with its effects') to one writer.

use makepad_editor_panel::make;
use makepad_score_live::model::ScoreDoc;
use makepad_score_live::source::fmt_num;

/// The panel's fixed widgets, before the instruments' tweaks.
pub const HEAD: [&str; 2] = ["ScoreTree{}", "SynthTrack{}"];

/// The word each writer's records start with.
pub const FIRST: &str = "SynthTweaks";

/// Writers a panel is made by, at once: about one per this many live
/// paths, from three to six.
const PATHS_PER_PART: usize = 16;
const PANEL_PARTS: (usize, usize) = (3, 6);

/// What each writer is told (`app`: where the score is edited): rules, then the widgets.
pub fn maker_system(app: &str) -> String {
    let rules = format!("You write the tweaks of a score's instruments for its editor panel in {app} \
(live-coded music in Splash). The panel's skeleton is fixed and not yours: a tree of the voices (mute, solo, level, pan, \
the instrument switch) and the selected voice's timeline and melody variants. You write, for each instrument you are \
given, its detailed controls, shown when it is selected: answer only with one `SynthTweaks{{node: \"<instrument>\" \
PanelModule{{title: \"…\" …}}}}` record per instrument (Splash, as in the catalogues below): no `let`, no outer View, \
no prose, no code fences, no comments. Inside: its pictures (SynthFilter, SynthWave, PanelEnvelope over its attack, \
decay, sustain and release), a PanelRow of knobs for its sound, its effects' knobs, a PanelMacro for the gesture a \
player wants, SynthKeys to try it. No SynthPicker (the tree switches instruments), no level or pan (the tree has them). \
Bind only paths marked live, ranges around the values written. Each widget on one line, only the fields it needs.");
    let c = crate::CATALOGUE;
    let head = c.split("## What sounds live").next().unwrap_or("");
    let tweaks = c.find("## An instrument's tweaks").map_or("", |at| &c[at..]);
    format!("{rules}\n\n{}\n{head}\n{tweaks}", makepad_value_pickers::panel::CATALOGUE)
}

fn type_of(doc: &ScoreDoc, node: &str) -> Option<String> {
    doc.order.iter().find(|(n, _)| n == node).map(|(_, t)| t.clone())
}

/// The writers' messages for a score whose bindable paths are `listing`
/// ([`makepad_editor_panel::listing`], each line marked `live` where a held
/// value plays at once): the live paths grouped by instrument (an effect
/// with the instrument of the first voice it takes in; a Synth that writes
/// few settings gets the ones a panel holds, at their defaults), split
/// into parts. Also the live paths' count.
pub fn part_texts(listing: &str, doc: &ScoreDoc, brief: Option<&str>) -> Result<(Vec<String>, usize), String> {
    // The voices' level, pan, mute and solo are the tree's.
    let live: Vec<&str> = listing.lines().filter(|l| l.contains(" live")).filter(|l| doc.loop_def(l.split(['.', ' ']).next().unwrap_or("")).is_none()).collect();
    let owner = |node: &str| -> Option<String> {
        if doc.inst(node).is_some() {
            return Some(node.to_string());
        }
        let fx = doc.fx.iter().find(|f| f.name == node)?;
        fx.inputs.iter().find_map(|i| doc.loop_def(i).and_then(|l| l.play.clone()))
    };
    let voices = |inst: &str| doc.loops.iter().filter(|l| l.play.as_deref() == Some(inst)).map(|l| l.name.clone()).collect::<Vec<_>>();
    let mut groups: Vec<(String, String)> = Vec::new();
    for line in &live {
        let Some(key) = owner(line.split(['.', ' ']).next().unwrap_or("")) else { continue };
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, text)) => {
                text.push_str(line);
                text.push('\n');
            }
            None => groups.push((key.clone(), format!("instrument `{key}` (played by {}):\n{line}\n", voices(&key).join(", ")))),
        }
    }
    for inst in &doc.insts {
        if type_of(doc, &inst.name).as_deref() != Some("Synth") {
            continue;
        }
        let written = groups.iter().find(|(k, _)| *k == inst.name).map_or(0, |(_, t)| t.lines().count().saturating_sub(1));
        if written >= 4 {
            continue;
        }
        let mut extra = String::new();
        for key in ["cutoff", "res", "attack", "decay", "sustain", "release", "drive", "gain"] {
            let path = format!("{}.{key}", inst.name);
            if listing.lines().any(|l| l.starts_with(&format!("{path} "))) {
                continue;
            }
            if let Some(spec) = crate::spec::spec("Synth", key) {
                extra.push_str(&format!("{path} (number live {}..{}) = {}  // not written yet: its preset's\n", fmt_num(spec.min, 4), fmt_num(spec.max, 4), fmt_num(spec.default, 4)));
            }
        }
        match groups.iter_mut().find(|(k, _)| *k == inst.name) {
            Some((_, text)) => text.push_str(&extra),
            None => {
                let v = voices(&inst.name);
                if !v.is_empty() {
                    groups.push((inst.name.clone(), format!("instrument `{}` (played by {}):\n{extra}", inst.name, v.join(", "))));
                }
            }
        }
    }
    if groups.is_empty() {
        return Err("no instrument of this score has settings that play live: the voice tree is its editor".into());
    }
    let texts: Vec<String> = groups.into_iter().map(|(_, t)| t).collect();
    let k = make::part_count(live.len(), PATHS_PER_PART, PANEL_PARTS).min(texts.len());
    let parts = make::split_groups(&texts, k);
    // Every writer knows the score's nodes (what plays what).
    let mut nodes = String::new();
    for (name, ty) in &doc.order {
        let plays = doc.loop_def(name).and_then(|l| l.play.clone()).map(|p| format!(" plays {p}")).unwrap_or_default();
        nodes.push_str(&format!("{name}: {ty}{plays}\n"));
    }
    let label = if doc.label.is_empty() { "the score".to_string() } else { doc.label.clone() };
    let steer = make::steer(brief);
    let count = parts.len();
    let texts = parts.into_iter().enumerate().map(|(i, part)| format!("The score: {label}. Its nodes:\n{nodes}\nWrite each instrument's tweaks for these paths (part {} of {count}):{steer}\n{part}", i + 1)).collect();
    Ok((texts, live.len()))
}
