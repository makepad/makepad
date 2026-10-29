//! The score's player: the typed score built straight into the audio
//! engine's owner slice (data, node for node: no source text, no flow
//! graph) and prepared into a schedule.
//!
//! One `Loop` per loop with its instrument's settings, a `Loop` and an FX
//! per `fx()` scope (the deepest first), a `Bus` per aux send, one FX node
//! per FX (with its key), unit Buses summing more than [`FAN_IN`] inputs,
//! and the master `Limiter`. The clock is the player's own `Transport`
//! (`<name>_clock`, out through an `AudioOutput`: offline renders, games,
//! Synth) or its host's, given pass by pass ([`ClockMode::Host`]: the
//! `Score` node of any audio flow graph, [`crate::node`]).
//!
//! Node ids are `<name>/<node>`, so a loop's feed is `<name>/<loop>` and an
//! FX's automation `<name>/<fx>` whoever plays it.

use crate::model::{scope_fx, scope_play, Diag, InstKind, ListKind, LoopDef, ScoreDoc};
use makepad_audio_engine::feed::{loop_feed, LoopFeed};
use makepad_audio_engine::nodes::{descriptor, CLOCK};
use makepad_audio_engine::run::{run, NoExt};
use makepad_audio_engine::schedule::{compile, AudioBudget, ClockChunk, Layout, NodeState, Schedule, CLOCK_IN, SPAN};
use makepad_audio_engine::{CHUNK, OWNER};
use makepad_flow::document::ident::DPort;
use makepad_flow::document::types::TypeKey;
use makepad_flow::document::TypeDescriptor;
use makepad_flow::live::plan::{dnode, qualified, LiveInput, LiveNode, LiveSource, OwnerSlice};
use makepad_flow::{Literal, PortType};
use makepad_score_player::presets::{recipe_text, SLOTS};
use makepad_score_player::FxKind;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Whose clock the player follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockMode {
    /// Its own `Transport` (`<name>_clock`, playing at the score's tempo),
    /// out through an open `AudioOutput`.
    Own,
    /// The clock its host hands each pass ([`Player::run_on`]); its output
    /// is the master's span.
    Host,
}

/// The transport of a player on its own clock named `name` (its tempo
/// feed is where the composer's tempo map goes).
pub fn transport(name: &str) -> String {
    format!("{name}_clock")
}

/// The most entries a node's audio input lists; more are summed through
/// unit Buses first (`<to>_mix_<level>_<n>`, in a tree).
pub const FAN_IN: usize = 16;

/// The name a library sample (or file, or singer) is registered under
/// (`makepad_audio_engine::feed::samples`): `id@rev`.
pub fn sample_name(asset: &crate::model::AssetRef) -> String {
    format!("{}@{}", asset.id, asset.rev)
}

fn num(value: f64) -> Literal {
    Literal::Num(value)
}

fn text(value: &str) -> Literal {
    Literal::Str(value.to_string())
}

fn key_of(ty: &PortType) -> TypeKey {
    match ty {
        PortType::Registered(key) => key.clone(),
        other => TypeKey::new(&format!("{other:?}")),
    }
}

/// A shader node's code and settings.
fn shader_fields(shader: &crate::model::ShaderDef) -> [(&'static str, Literal); 2] {
    let params = shader.params.iter().flat_map(|(name, value)| [text(name), num(*value as f64)]).collect();
    [("shader", text(&shader.code)), ("shader_params", Literal::Arr(params))]
}

/// The instrument settings of a loop's node.
fn inst_fields(doc: &ScoreDoc, l: &LoopDef) -> Vec<(&'static str, Literal)> {
    let inst = l.play.as_deref().and_then(|name| doc.inst(name));
    let style = |values: &[f32]| Literal::Arr(values.iter().map(|v| num(if v.is_finite() { *v as f64 } else { -1.0 })).collect());
    match inst.map(|i| &i.kind) {
        Some(InstKind::Synth { recipe, mono }) => vec![("inst", text("recipe")), ("recipe", text(&recipe_text(recipe))), ("mono", num(*mono as u8 as f64))],
        Some(InstKind::Drums { slots, .. }) => vec![
            ("inst", text("kit")),
            ("kit", Literal::Arr(slots.iter().map(|s| text(&s.recipe.as_ref().map(recipe_text).unwrap_or_default())).collect())),
            ("kit_gain", Literal::Arr(slots.iter().map(|s| num(s.gain as f64)).collect())),
            ("kit_pitch", Literal::Arr(slots.iter().map(|s| num(2f64.powf(s.tune as f64 / 12.0))).collect())),
            ("kit_sample", Literal::Arr(slots.iter().map(|s| text(&s.sample.as_ref().map(sample_name).unwrap_or_default())).collect())),
        ],
        Some(InstKind::Sampler { src, root, attack, release }) => vec![
            ("inst", text("sampler")), ("sample", text(&sample_name(src))), ("root", num(*root as f64)), ("attack", num(*attack as f64)), ("release", num(*release as f64)),
        ],
        Some(InstKind::Voice { profile, singers, parts, style: values, neural, .. }) => vec![
            ("inst", text("voice")), ("engine", text(if *neural { "neural" } else { "dsp" })), ("profile", text(profile)),
            ("singers", num(*singers as f64)), ("parts", num(*parts as f64)), ("style", style(values)),
        ],
        Some(InstKind::Native { native, preset, font, bank, program, gain }) => {
            // The programs it plays: its own, then each change in the list.
            let mut programs = vec![*program];
            for p in l.notes.iter().flat_map(|notes| notes.iter()).filter_map(|n| match n.kind { ListKind::Program(p) => Some(p), _ => None }) {
                if !programs.contains(&p) {
                    programs.push(p);
                }
            }
            let font = match font {
                Some(font) => sample_name(font),
                None if native == "soundfont" => crate::gm_font::FONT.to_string(),
                None => String::new(),
            };
            vec![
                ("inst", text("native")), ("native", text(native)), ("preset", text(preset.as_deref().unwrap_or(""))), ("font", text(&font)),
                ("bank", num(*bank as f64)), ("programs", Literal::Arr(programs.iter().map(|p| num(*p as f64)).collect())), ("gain", num(*gain as f64)),
            ]
        }
        Some(InstKind::Input { gain }) => vec![("inst", text("input")), ("gain", num(*gain as f64))],
        Some(InstKind::Midi { channel, port }) => vec![("inst", text("midi")), ("channel", num(*channel as f64)), ("midi_port", text(port))],
        Some(InstKind::Shader { shader, mono }) => {
            let mut out = vec![("inst", text("shader"))];
            out.extend(shader_fields(shader));
            out.push(("mono", num(*mono as u8 as f64)));
            out
        }
        Some(InstKind::Morph { patch, srcs }) => vec![
            ("inst", text("morph")), ("morph", text(patch)),
            ("morph_src", Literal::Arr(srcs.iter().map(|s| text(&s.as_ref().map(sample_name).unwrap_or_default())).collect())),
        ],
        Some(InstKind::Tandem { path, set, .. }) => vec![
            ("inst", text("tandem")), ("tandem_path", text(path)),
            ("tandem_set", text(&set.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "))),
        ],
        Some(InstKind::Ironfish { program, values }) => vec![("inst", text("ironfish")), ("program", num(*program as f64)), ("iron", style(values))],
        None => {
            // A drum pattern without an instrument plays the default kit.
            let sounds = crate::palette::sounds::sounds();
            let kit = sounds.kit(crate::palette::sounds::DEFAULT_KIT).map(|k| k.slots.clone()).unwrap_or_else(|| vec![None; SLOTS]);
            vec![("inst", text("kit")), ("kit", Literal::Arr(kit.iter().map(|r| text(&r.as_ref().map(recipe_text).unwrap_or_default())).collect()))]
        }
    }
}

/// A node of type `descriptor` as an owner slice holds it: every
/// parameter with its authored value or its default, every input with what
/// is wired into it (`(node id, port)`, in entry order). How a host builds
/// a slice as data.
pub fn live_node(descriptor: &TypeDescriptor, id: String, authored: &[(&str, Literal)], wires: &[(&str, Vec<(String, &str)>)]) -> LiveNode {
    let params = descriptor.params.iter().map(|param| {
        let value = authored.iter().find(|(name, _)| *name == param.name).map(|(_, value)| value.clone());
        (param.name.clone(), value.unwrap_or_else(|| param.default.clone()))
    }).collect();
    let inputs = descriptor.inputs.iter().map(|input| {
        let from = wires.iter().find(|(port, _)| *port == input.name).map(|(_, from)| from.as_slice()).unwrap_or_default();
        let sources = from.iter().enumerate().map(|(entry, (node, port))| LiveSource {
            from: DPort { node: dnode(node), port: port.to_string() },
            conn: input.many.map(|_| entry as u64 + 1),
            via: Vec::new(),
            lineage: Vec::new(),
        }).collect();
        LiveInput { port: input.name.clone(), ty: key_of(&input.ty), many: input.many, sources, control: None }
    }).collect();
    let outputs = descriptor.outputs.iter().map(|output| (output.name.clone(), key_of(&output.ty))).collect();
    LiveNode { node: dnode(&id), id, type_name: descriptor.type_name.clone(), params, inputs, outputs }
}

/// The slice being built: nodes in the order the graph declares them.
struct Build<'a> {
    name: &'a str,
    nodes: Vec<LiveNode>,
}

impl Build<'_> {
    fn id(&self, member: &str) -> String {
        format!("{}/{member}", self.name)
    }

    fn node(&mut self, type_name: &str, id: String, authored: &[(&str, Literal)], wires: &[(&str, Vec<(String, &str)>)]) {
        self.nodes.push(live_node(descriptor(type_name).expect("an audio engine node type"), id, authored, wires));
    }

    /// `inputs` (member names) as at most [`FAN_IN`] node ids.
    fn fan_in(&mut self, to: &str, inputs: Vec<String>) -> Vec<String> {
        let mut inputs: Vec<String> = inputs.iter().map(|name| self.id(name)).collect();
        let mut level = 0;
        while inputs.len() > FAN_IN {
            level += 1;
            inputs = inputs.chunks(FAN_IN).enumerate().map(|(k, chunk)| {
                let bus = self.id(&format!("{to}_mix_{level}_{}", k + 1));
                let wires = [("audio", chunk.iter().map(|id| (id.clone(), "audio")).collect())];
                self.node("Bus", bus.clone(), &[("gain", num(1.0)), ("pan", num(0.0))], &wires);
                bus
            }).collect();
        }
        inputs
    }
}

/// The score as the audio owner's slice, its nodes `<name>/…`.
pub fn slice(doc: &ScoreDoc, name: &str, clocking: ClockMode) -> OwnerSlice {
    let mut b = Build { name, nodes: Vec::new() };
    let clock = match clocking {
        ClockMode::Own => {
            let clock = transport(name);
            b.node("Transport", clock.clone(), &[("bpm", num(doc.clock.bpm.clamp(40.0, 240.0))), ("playing", num(1.0))], &[]);
            clock
        }
        ClockMode::Host => {
            let clock = b.id("clock");
            b.nodes.push(LiveNode {
                node: dnode(&clock), id: clock.clone(), type_name: CLOCK_IN.into(), params: Vec::new(), inputs: Vec::new(),
                outputs: vec![("clock".into(), TypeKey::new(CLOCK))],
            });
            clock
        }
    };
    let clocked = [("clock", vec![(clock.clone(), "clock")])];
    let audio = |ids: &[String]| [("audio", ids.iter().map(|id| (id.clone(), "audio")).collect::<Vec<_>>())];
    for l in &doc.loops {
        let mut fields = inst_fields(doc, l);
        fields.extend([("amp", num(l.amp as f64)), ("pan", num(l.pan as f64)), ("mute", num(l.mute as u8 as f64)), ("solo", num(l.solo as u8 as f64))]);
        b.node("Loop", b.id(&l.name), &fields, &clocked);
        // fx() scopes: a copy of the loop playing the scope's notes into
        // its effect, the deepest first (an effect reads its inner ones).
        let mut order: Vec<usize> = (0..l.scopes.len()).collect();
        order.sort_by_key(|&k| std::cmp::Reverse(l.scopes[k].len()));
        for k in order {
            let chain = &l.scopes[k];
            let (play, fx) = (b.id(&scope_play(&l.name, k + 1)), b.id(&scope_fx(&l.name, k + 1)));
            b.node("Loop", play.clone(), &fields, &clocked);
            let inner = l.scopes.iter().enumerate().filter(|(_, c)| c.len() == chain.len() + 1 && c.starts_with(chain)).map(|(j, _)| b.id(&scope_fx(&l.name, j + 1)));
            let entries: Vec<String> = std::iter::once(play).chain(inner).collect();
            let kind = *chain.last().unwrap_or(&FxKind::Bus);
            let params: Vec<(&str, Literal)> = kind.params().iter().map(|(param, default, ..)| (*param, num(*default as f64))).collect();
            b.node(kind.name(), fx, &params, &audio(&entries));
        }
    }
    // What a loop's audio is: the loop and its outermost fx() scopes.
    let outs = |name: &str| -> Vec<String> {
        let mut v = vec![name.to_string()];
        if let Some(l) = doc.loops.iter().find(|l| l.name == name) {
            v.extend(l.scopes.iter().enumerate().filter(|(_, c)| c.len() == 1).map(|(k, _)| scope_fx(&l.name, k + 1)));
        }
        v
    };
    let mut consumed: Vec<String> = Vec::new();
    // Aux sends: a Bus per (loop, FX) at the send's amount, read by the FX;
    // the loop keeps its own path.
    let mut sends: Vec<(String, String)> = Vec::new();
    for l in &doc.loops {
        for (fx, amount) in &l.sends {
            let bus = format!("{}_send_{}", l.name, fx);
            let entries: Vec<String> = outs(&l.name).iter().map(|name| b.id(name)).collect();
            b.node("Bus", b.id(&bus), &[("gain", num((*amount as f64).min(2.0))), ("pan", num(0.0))], &audio(&entries));
            consumed.push(bus.clone());
            sends.push((fx.clone(), bus));
        }
    }
    for fx in &doc.fx {
        let inputs: Vec<String> = fx.inputs.iter().flat_map(|i| outs(i)).chain(sends.iter().filter(|(to, _)| *to == fx.name).map(|(_, bus)| bus.clone())).collect();
        let inputs = b.fan_in(&fx.name, inputs);
        consumed.extend(fx.inputs.iter().flat_map(|i| outs(i)));
        let id = b.id(&fx.name);
        match &fx.shader {
            Some(shader) => b.node("ShaderFx", id, &shader_fields(shader), &audio(&inputs)),
            None => {
                let params: Vec<(&str, Literal)> = fx.kind.params().iter().enumerate().map(|(i, (param, ..))| (*param, num(fx.params[i] as f64))).collect();
                let mut wires = vec![("audio", inputs.iter().map(|id| (id.clone(), "audio")).collect::<Vec<_>>())];
                if let (Some(side), FxKind::Duck | FxKind::Comp) = (&fx.side, fx.kind) {
                    wires.push(("key", vec![(b.id(side), "audio")]));
                }
                b.node(fx.kind.name(), id, &params, &wires);
            }
        }
    }
    // Everything nothing else reads, through the master limiter.
    let to_master: Vec<String> = doc.loops.iter().flat_map(|l| outs(&l.name)).chain(doc.fx.iter().map(|f| f.name.clone())).filter(|name| !consumed.iter().any(|c| c == name)).collect();
    let to_master = b.fan_in("master", to_master);
    let master = b.id("master");
    b.node("Limiter", master.clone(), &[("ceiling", num(doc.ceiling as f64)), ("gain", num(doc.master as f64))], &audio(&to_master));
    if clocking == ClockMode::Own {
        b.node("AudioOutput", "out".into(), &[("enabled", num(1.0))], &audio(&[master]));
    }
    OwnerSlice { owner: OWNER.into(), nodes: topological(b.nodes) }
}

/// Each node after every node it reads: ready nodes taken depth first from
/// the declaration order (the order the flow host's partition gives a slice).
fn topological(nodes: Vec<LiveNode>) -> Vec<LiveNode> {
    let index: HashMap<String, usize> = nodes.iter().enumerate().map(|(index, node)| (node.id.clone(), index)).collect();
    let reads = |node: &LiveNode| node.inputs.iter().flat_map(|input| &input.sources).map(|source| qualified(&source.from.node)).collect::<Vec<_>>();
    let mut pending: Vec<usize> = nodes.iter().map(|node| reads(node).iter().filter(|id| index.contains_key(*id)).count()).collect();
    let mut order = Vec::with_capacity(nodes.len());
    let mut ready: Vec<usize> = (0..nodes.len()).filter(|index| pending[*index] == 0).collect();
    ready.reverse();
    while let Some(next) = ready.pop() {
        order.push(next);
        for (other, node) in nodes.iter().enumerate() {
            let count = reads(node).iter().filter(|id| **id == nodes[next].id).count();
            if count > 0 {
                pending[other] -= count;
                if pending[other] == 0 {
                    ready.push(other);
                }
            }
        }
    }
    let mut slots: Vec<Option<LiveNode>> = nodes.into_iter().map(Some).collect();
    order.into_iter().filter_map(|index| slots[index].take()).collect()
}

/// What a player on its host's clock publishes for the host to read (the
/// meters of Stage's SCORE page): its probes and its streams' last pass.
pub struct Meters {
    probes: Box<[AtomicU32]>,
    /// Per audio span, [`SCOPE_BINS`] of (left min, left max, right min,
    /// right max), f32 bits.
    scopes: Box<[AtomicU32]>,
    spans: usize,
}

/// Bins of a stream's published pass.
pub const SCOPE_BINS: usize = 128;
/// Streams whose scope one pass publishes.
const SCOPES_PER_PASS: usize = 2;

impl Meters {
    fn new(probes: usize, spans: usize) -> Meters {
        Meters {
            probes: (0..probes).map(|_| AtomicU32::new(0)).collect(),
            scopes: (0..spans * SCOPE_BINS * 4).map(|_| AtomicU32::new(0)).collect(),
            spans,
        }
    }

    pub fn probe(&self, index: u16) -> f32 {
        self.probes.get(index as usize).map_or(0.0, |v| f32::from_bits(v.load(Ordering::Relaxed)))
    }

    /// [`Meters::scope`] into `out` (up to [`SCOPE_BINS`] bins), without
    /// allocating.
    pub fn scope_into(&self, span: u16, out: &mut [[f32; 4]]) {
        let base = span as usize * SCOPE_BINS * 4;
        for (bin, slot) in out.iter_mut().take(SCOPE_BINS).enumerate() {
            for (k, v) in slot.iter_mut().enumerate() {
                *v = self.scopes.get(base + bin * 4 + k).map_or(0.0, |a| f32::from_bits(a.load(Ordering::Relaxed)));
            }
        }
    }

    /// A stream's last published pass: per bin (left min, left max, right
    /// min, right max).
    pub fn scope(&self, span: u16) -> Vec<[f32; 4]> {
        let base = span as usize * SCOPE_BINS * 4;
        (0..SCOPE_BINS).map(|bin| std::array::from_fn(|k| self.scopes.get(base + bin * 4 + k).map_or(0.0, |v| f32::from_bits(v.load(Ordering::Relaxed))))).collect()
    }
}

/// A score prepared on the engine: its schedule, a feed per loop.
pub struct Player {
    pub schedule: Box<Schedule<(), ()>>,
    pub layout: Arc<Layout>,
    pub feeds: Vec<(u64, Arc<LoopFeed>)>,
    pub name: String,
    pub clocking: ClockMode,
    /// What it was prepared from (a host prepares again only when it changed).
    pub slice: OwnerSlice,
    pub meters: Option<Arc<Meters>>,
    master: u16,
    scope_cursor: usize,
}

fn fail(message: impl Into<String>) -> Vec<Diag> {
    vec![Diag { line: 1, col: 1, node: None, message: message.into() }]
}

/// Prepare `doc` as the player `name` at `rate`, generation `generation`;
/// against `base` (the layout playing) its staying nodes list their state
/// to carry over (`Schedule::migrate`).
pub fn prepare(doc: &ScoreDoc, rate: f32, name: &str, clocking: ClockMode, base: Option<&Layout>, generation: u64) -> Result<Player, Vec<Diag>> {
    let slice = slice(doc, name, clocking);
    prepare_slice(doc, slice, rate, name, clocking, base, generation)
}

/// [`prepare`] from a slice already built.
pub(crate) fn prepare_slice(doc: &ScoreDoc, slice: OwnerSlice, rate: f32, name: &str, clocking: ClockMode, base: Option<&Layout>, generation: u64) -> Result<Player, Vec<Diag>> {
    let format = (1u64 << 32) | rate.round() as u64;
    let compiled = compile::<NoExt, ()>(&slice, generation, 1, format, base, &Arc::new(AudioBudget::default())).map_err(|e| fail(e.message))?;
    let feeds = doc.feed_names().iter().map(|n| (crate::key(n), loop_feed(&format!("{name}/{n}")))).collect();
    let master = *compiled.layout.audio_probes.get(&format!("{name}/master.audio")).ok_or_else(|| fail("the score has no master"))?;
    let meters = (clocking == ClockMode::Host).then(|| Arc::new(Meters::new(compiled.schedule.probes.len(), compiled.schedule.arena.len() / SPAN)));
    Ok(Player { schedule: compiled.schedule, layout: compiled.layout, feeds, name: name.to_string(), clocking, slice, meters, master, scope_cursor: 0 })
}

impl Player {
    /// One pass of `frames` (at most [`CHUNK`]) on its own clock; `sink`
    /// gets what its output sends.
    pub fn run(&mut self, frames: usize, sink: &mut dyn FnMut(bool, &[f32], &[f32])) {
        run(&mut self.schedule, frames, sink, &mut NoExt);
    }

    /// One pass on the host's `clock`: `left` and `right` (the pass's
    /// frames) get the master. No allocation, lock or free.
    pub fn run_on(&mut self, clock: &ClockChunk, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len()).min(CHUNK);
        self.schedule.drive_clock(0, *clock);
        run(&mut self.schedule, frames, &mut |_, _, _| {}, &mut NoExt);
        let span = &self.schedule.arena[self.master as usize * SPAN..][..SPAN];
        left[..frames].copy_from_slice(&span[..frames]);
        right[..frames].copy_from_slice(&span[CHUNK..CHUNK + frames]);
        self.publish(frames);
    }

    /// The pass's probes and a few streams' scopes, for the host.
    fn publish(&mut self, frames: usize) {
        let Some(meters) = &self.meters else { return };
        for (slot, value) in meters.probes.iter().zip(self.schedule.probes.iter()) {
            slot.store(value.to_bits(), Ordering::Relaxed);
        }
        if meters.spans == 0 || frames == 0 {
            return;
        }
        for _ in 0..SCOPES_PER_PASS.min(meters.spans) {
            let span_index = self.scope_cursor % meters.spans;
            self.scope_cursor = self.scope_cursor.wrapping_add(1);
            let span = &self.schedule.arena[span_index * SPAN..][..SPAN];
            let base = span_index * SCOPE_BINS * 4;
            for bin in 0..SCOPE_BINS {
                let start = bin * frames / SCOPE_BINS;
                let end = ((bin + 1) * frames / SCOPE_BINS).max(start + 1).min(frames);
                let mut bounds = [span[start], span[start], span[CHUNK + start], span[CHUNK + start]];
                for at in start..end {
                    bounds[0] = bounds[0].min(span[at]);
                    bounds[1] = bounds[1].max(span[at]);
                    bounds[2] = bounds[2].min(span[CHUNK + at]);
                    bounds[3] = bounds[3].max(span[CHUNK + at]);
                }
                for (k, value) in bounds.iter().enumerate() {
                    meters.scopes[base + bin * 4 + k].store(value.to_bits(), Ordering::Relaxed);
                }
            }
        }
    }

    /// Whether anything plays or waits to (a loop sounding on a running
    /// clock, events or automation in a feed, an open input).
    pub fn busy(&self) -> bool {
        self.schedule.states.iter().any(|state| match state {
            NodeState::Loop(this) => this.playing || this.input.is_some() || this.feed.as_ref().is_some_and(|feed| !feed.is_empty()),
            NodeState::Fx(this) => this.feed.as_ref().is_some_and(|feed| !feed.is_empty()),
            _ => false,
        })
    }

    /// The bank slot of `<node>.<param>` (`node` without the player's name).
    pub fn control(&self, node: &str, param: &str) -> Option<u16> {
        self.layout.controls.get(&format!("{}/{node}.{param}", self.name)).map(|(index, _)| *index)
    }
}

/// Hold `value` on a prepared player's targets (its loops' live settings,
/// its controls), as an editor's controls do.
pub fn hold(player: &Player, targets: &[crate::held::Target], value: f32) {
    for target in targets {
        match target {
            crate::held::Target::Params { node, key } => makepad_audio_engine::feed::live_params(&format!("{}/{node}", player.name)).set(key, value),
            crate::held::Target::Control { node, param } => {
                if let Some(index) = player.control(node, param) {
                    player.schedule.controls.set_manual(index, value);
                }
            }
        }
    }
}
