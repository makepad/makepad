//! The audio node types. Every parameter says what it is: an authored
//! setting (a source edit, a new revision) or an endpoint a running plan
//! admits (a control, a toggle, a trigger), with its range and description.

use makepad_flow::document::types::{Carrier, Rate, TypeKey, TypeSchema};
use makepad_flow::document::{EndpointClass, InputDescriptor, ParamDescriptor, ParamType, Registry, TypeDescriptor};
use makepad_flow::{Literal, Port, PortType};

pub const OWNER: &str = "stage.audio";
pub const AUDIO: &str = "stage.audio";
pub const CLOCK: &str = "stage.clock";
pub const LEVEL: &str = "stage.level";

pub fn ty(key: &str) -> PortType {
    PortType::Registered(TypeKey::new(key))
}

/// An ordered input takes as many streams as a flow orders (its own
/// bound on ordered entries): the schedule sizes its sums to what is wired.
const ANY: u32 = makepad_flow::MAX_SEQUENCE;

pub fn input(name: &str, key: &str, many: Option<u32>) -> InputDescriptor {
    InputDescriptor {
        name: name.into(), ty: ty(key), many,
        default: if many.is_some() { Literal::Arr(Vec::new()) } else { Literal::Null },
        flexible: false,
    }
}

pub fn output(name: &str, key: &str) -> Port {
    Port { name: name.into(), ty: ty(key), many: None }
}

pub fn node(name: &str, doc: &str, params: Vec<ParamDescriptor>, inputs: Vec<InputDescriptor>, outputs: Vec<Port>) -> TypeDescriptor {
    let mut descriptor = TypeDescriptor::provider(name, OWNER, inputs, outputs);
    // Every control and toggle also takes a same-owner audio scalar wire.
    descriptor.params = params.into_iter().map(|param| match param.endpoint {
        Some(EndpointClass::Control | EndpointClass::Toggle) => param.with_signal(ty(LEVEL)),
        _ => param,
    }).collect();
    descriptor.doc = doc.into();
    descriptor
}

/// Register the audio types and the engine's own nodes. An owner adds its
/// own node types and names itself live (see live_audio's `register`).
pub fn register_nodes(registry: &mut Registry) -> Result<(), String> {
    let schema = |key: &str, carrier, rate, doc: &str| TypeSchema {
        key: TypeKey::new(key), schema_rev: 1, carrier, rate, local_only: true,
        serial: None, audio_lowering: None, doc: doc.into(),
    };
    for schema in [
        schema(AUDIO, Carrier::AudioPort, Rate::Block, "A stereo audio stream inside the audio owner. It never leaves the device callback's schedule."),
        schema(CLOCK, Carrier::Clock, Rate::Block, "Beat position, tempo and whether the transport plays, counted in device frames."),
        schema(LEVEL, Carrier::Control, Rate::Control, "A number an audio node publishes: a peak, a step, a count."),
    ] {
        registry.register_type(schema).map_err(|error| error.to_string())?;
    }
    for descriptor in descriptors() {
        registry.register(descriptor.clone()).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The engine's own node types, made once: what [`register_nodes`]
/// registers, and what a player that builds its owner slice as data
/// (the score's) reads every parameter's default from.
pub fn descriptors() -> &'static [TypeDescriptor] {
    static DESCRIPTORS: std::sync::OnceLock<Vec<TypeDescriptor>> = std::sync::OnceLock::new();
    DESCRIPTORS.get_or_init(build_descriptors)
}

/// One of [`descriptors`] by type name.
pub fn descriptor(type_name: &str) -> Option<&'static TypeDescriptor> {
    descriptors().iter().find(|descriptor| descriptor.type_name == type_name)
}

fn build_descriptors() -> Vec<TypeDescriptor> {
    let mut stem_params = Vec::new();
    for stem in ["vocals", "drums", "bass", "other"] {
        stem_params.push(ParamDescriptor::control(format!("gain_{stem}"), 1.0, 0.0, 1.5, "Linear gain of this measured stem; 1 is unity."));
        stem_params.push(ParamDescriptor::toggle(format!("mute_{stem}"), false, "Silence this stem without changing its gain."));
        stem_params.push(ParamDescriptor::toggle(format!("solo_{stem}"), false, "When any stem is soloed, only unmuted soloed stems sound."));
    }
    let descriptors = [
        node(
            "Transport",
            "The musical clock of an audio graph: tempo and play state, counted in device frames. Wire `clock` into every Sequencer that should follow it.",
            vec![
                ParamDescriptor::control("bpm", 120.0, 40.0, 240.0, "Tempo in beats per minute."),
                ParamDescriptor::toggle("playing", false, "Whether the clock advances. Stopping holds the position."),
            ],
            vec![],
            vec![output("clock", CLOCK)],
        ),
        node(
            "Filter",
            "A two-pole state-variable low-pass filter on a stereo stream.",
            vec![
                ParamDescriptor::control("cutoff", 1200.0, 40.0, 18000.0, "Cutoff frequency in hertz."),
                ParamDescriptor::control("resonance", 0.7, 0.1, 8.0, "Filter Q: 0.7 is flat, higher rings."),
            ],
            vec![input("audio", AUDIO, None)],
            vec![output("audio", AUDIO)],
        ),
        node(
            "Gain",
            "Level and stereo position of a stream, ramped so a move never clicks. Mute preserves its gain; peak measures the resulting stereo signal.",
            vec![
                ParamDescriptor::control("gain", 0.8, 0.0, 2.0, "Linear gain; 1 is unity."),
                ParamDescriptor::control("pan", 0.0, -1.0, 1.0, "Stereo position, equal power: -1 left, 1 right."),
                ParamDescriptor::toggle("mute", false, "Ramp this stream to silence without changing its gain or stereo position."),
            ],
            vec![input("audio", AUDIO, None)],
            vec![output("audio", AUDIO), output("peak", LEVEL)],
        ),
        node(
            "AudioMixer",
            "Sums an ordered list of streams, as many as are wired; the order is the order of the entries.",
            vec![ParamDescriptor::control("master", 0.8, 0.0, 2.0, "Linear gain of the sum.")],
            vec![input("inputs", AUDIO, Some(ANY))],
            vec![output("audio", AUDIO)],
        ),
        node(
            "StemMixer",
            "Mixes four synchronized measured stems. With no complete stem source, passes the original mixed audio unchanged. Gain, mute and solo are ramped.",
            stem_params,
            vec![input("audio", AUDIO, None), input("vocals", AUDIO, None), input("drums", AUDIO, None), input("bass", AUDIO, None), input("other", AUDIO, None), input("available", LEVEL, None)],
            vec![output("audio", AUDIO)],
        ),
        node(
            "AudioOutput",
            "Sends a stream to the audio device. The gate is closed until `enabled` is written; the peaks are measured before the gate, so a closed gate still shows what would sound.",
            vec![ParamDescriptor::toggle("enabled", false, "Open the gate to the device. Closed by default.")],
            vec![input("audio", AUDIO, None)],
            vec![output("peak_l", LEVEL), output("peak_r", LEVEL)],
        ),
        node(
            "Pads",
            "One-shot pads: a hit plays its pad's sample once from the start, over whatever still sounds (a pool of voices; when every voice sounds the oldest gives way). The host sends the hits; a stop lets a pad's voices go.",
            vec![
                ParamDescriptor::authored("samples", ParamType::LiteralArray, Literal::Arr(Vec::new()), "Each pad's sample: a registered sample's name (empty: a silent pad)."),
                ParamDescriptor::authored("gains", ParamType::LiteralArray, Literal::Arr(Vec::new()), "Each pad's level (1 as recorded)."),
                ParamDescriptor::control("amp", 1.0, 0.0, 4.0, "The pads' level; 1 is unity."),
                ParamDescriptor::toggle("mute", false, "Silence the pads, their voices running on."),
            ],
            vec![],
            vec![output("audio", AUDIO), output("peak", LEVEL)],
        ),
        node(
            "CueOutput",
            "Sends a stream to the headphone cue: the device's channels 3-4, or one ear of a stereo device when the host splits it. Open by default; the peaks are measured before the gate.",
            vec![ParamDescriptor::toggle("enabled", true, "Open the gate to the cue.")],
            vec![input("audio", AUDIO, None)],
            vec![output("peak_l", LEVEL), output("peak_r", LEVEL)],
        ),
    ];
    descriptors.into_iter().chain(score_nodes()).collect()
}

/// The nodes a score lowers to: `Loop` (the sequencer: events from its
/// composer, played on its own instrument) and the FX.
fn score_nodes() -> Vec<TypeDescriptor> {
    use makepad_score_player::FxKind;
    let text = |name: &str, doc: &str| ParamDescriptor::authored(name, ParamType::String, Literal::Str(String::new()), doc);
    let list = |name: &str, doc: &str| ParamDescriptor::authored(name, ParamType::LiteralArray, Literal::Arr(Vec::new()), doc);
    let number = |name: &str, value: f64, doc: &str| ParamDescriptor::authored(name, ParamType::Number, Literal::Num(value), doc);
    let mut out = vec![node(
        "Loop",
        "A score's loop: the events its code or pattern plays, sample-exact on the clock wired into it, on its own instrument (a recipe synth, a drum kit or Ironfish). The events arrive from the score's composer.",
        vec![
            text("inst", "The instrument: recipe, kit, sampler, ironfish, shader, voice or native."),
            text("engine", "A voice's engine: dsp (the realtime singer) or neural (the neural voice, its phrases rendered ahead)."),
            text("profile", "A voice's singer: soprano, alto, tenor, baritone or bass."),
            number("singers", 1.0, "A voice's choir size (1 is a solo voice)."),
            number("parts", 1.0, "The harmony parts a choir splits into."),
            list("style", "A voice's style: vibrato vibrato_rate vibrato_delay portamento scoop breath drift jitter (-1 keeps the default)."),
            text("shader", "A shader instrument's code (an audio shader)."),
            list("shader_params", "A shader instrument's settings: name, value, name, value..."),
            text("recipe", "A recipe synth's patch (recipe text)."),
            list("kit", "A drum kit's sixteen slots, each a recipe."),
            list("kit_gain", "Each kit slot's gain."),
            list("kit_pitch", "Each kit slot's pitch multiplier."),
            list("kit_sample", "Each kit slot's sample (a registered sample's name; empty plays the recipe)."),
            text("sample", "A sampler's sample (a registered sample's name)."),
            text("native", "A native instrument: piano, drums (the modelled kit) or soundfont."),
            text("preset", "A native instrument's preset (the piano's)."),
            text("font", "A SoundFont's file (a registered file's name)."),
            number("bank", 0.0, "A SoundFont's bank (128: percussion)."),
            list("programs", "The General MIDI programs a SoundFont loop plays (the first until a change)."),
            number("gain", 1.0, "A native instrument's level."),
            number("root", 60.0, "A sampler's root note (plays the sample as recorded)."),
            number("attack", 0.002, "A sampler's attack (s)."),
            number("release", 0.05, "A sampler's release (s)."),
            number("mono", 0.0, "1 plays one voice at a time (legato lines)."),
            number("program", 0.0, "Ironfish: which built-in program (0-7)."),
            number("channel", 1.0, "A MIDI loop's channel (1-16)."),
            text("midi_port", "A MIDI loop's output (its name; empty: the first)."),
            text("morph", "Morph: its patch as text (key=value words, mod=src,dst,amount rows, fx=delay,reverb)."),
            text("tandem_path", "Tandem: the library patch file it plays (empty: the init patch)."),
            text("tandem_set", "Tandem: parameters set over the patch, `storage_name=value` words in raw units."),
            list("morph_src", "Morph: the samples its oscillators a, b, c read (registered sample names; empty: none)."),
            list("iron", "Ironfish: its settings, 0..1 each (-1 keeps the program's), in the score's order: cutoff resonance env attack decay sustain release sub noise glide crush delay chorus reverb output."),
            ParamDescriptor::control("amp", 0.8, 0.0, 4.0, "The loop's level; 1 is unity."),
            ParamDescriptor::control("pan", 0.0, -1.0, 1.0, "The loop's place in the stereo field."),
            ParamDescriptor::toggle("mute", false, "Silence the loop, keeping its place."),
            ParamDescriptor::toggle("solo", false, "When any loop is soloed only soloed loops sound."),
        ],
        vec![input("clock", CLOCK, None)],
        vec![output("audio", AUDIO), output("peak", LEVEL)],
    )];
    for kind in [
        FxKind::Lowpass, FxKind::Highpass, FxKind::Bandpass, FxKind::Delay, FxKind::Reverb, FxKind::Drive, FxKind::Crush, FxKind::Chorus,
        FxKind::Comp, FxKind::Duck, FxKind::Bus, FxKind::Limiter, FxKind::Eq, FxKind::Width, FxKind::Isolator, FxKind::DjFilter,
    ] {
        let params = kind.params().iter().map(|(name, default, lo, hi)| {
            if kind.is_toggle(name) {
                ParamDescriptor::toggle(*name, *default > 0.5, "An FX switch.")
            } else {
                ParamDescriptor::control(*name, *default as f64, *lo as f64, *hi as f64, "An FX setting.")
            }
        }).collect();
        let mut inputs = vec![input("audio", AUDIO, Some(ANY))];
        if matches!(kind, FxKind::Duck | FxKind::Comp) {
            inputs.push(input("key", AUDIO, None));
        }
        out.push(node(kind.name(), "A score's FX: the sum of its inputs, processed.", params, inputs, vec![output("audio", AUDIO), output("peak", LEVEL), output("gr", LEVEL)]));
    }
    out.push(node(
        "ShaderFx",
        "A score's FX written as an audio shader: the sum of its inputs through the shader.",
        vec![text("shader", "The effect's code (an audio shader)."), list("shader_params", "Its settings: name, value, name, value...")],
        vec![input("audio", AUDIO, Some(ANY))],
        vec![output("audio", AUDIO), output("peak", LEVEL)],
    ));
    out
}
