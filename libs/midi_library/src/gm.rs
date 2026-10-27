//! General MIDI: the 128 programs by name, the role each plays in an
//! arrangement, and the instrument a cover plays each role on (the role
//! table: the engine's production presets for the synth roles and the
//! electronic kits, the GM SoundFont for acoustic instruments).

/// The GM program names (program 0 is "Acoustic Grand Piano").
pub const PROGRAMS: [&str; 128] = [
    "Acoustic Grand Piano", "Bright Acoustic Piano", "Electric Grand Piano", "Honky-tonk Piano", "Electric Piano 1", "Electric Piano 2", "Harpsichord", "Clavinet",
    "Celesta", "Glockenspiel", "Music Box", "Vibraphone", "Marimba", "Xylophone", "Tubular Bells", "Dulcimer",
    "Drawbar Organ", "Percussive Organ", "Rock Organ", "Church Organ", "Reed Organ", "Accordion", "Harmonica", "Tango Accordion",
    "Acoustic Guitar (nylon)", "Acoustic Guitar (steel)", "Electric Guitar (jazz)", "Electric Guitar (clean)", "Electric Guitar (muted)", "Overdriven Guitar", "Distortion Guitar", "Guitar Harmonics",
    "Acoustic Bass", "Electric Bass (finger)", "Electric Bass (pick)", "Fretless Bass", "Slap Bass 1", "Slap Bass 2", "Synth Bass 1", "Synth Bass 2",
    "Violin", "Viola", "Cello", "Contrabass", "Tremolo Strings", "Pizzicato Strings", "Orchestral Harp", "Timpani",
    "String Ensemble 1", "String Ensemble 2", "Synth Strings 1", "Synth Strings 2", "Choir Aahs", "Voice Oohs", "Synth Voice", "Orchestra Hit",
    "Trumpet", "Trombone", "Tuba", "Muted Trumpet", "French Horn", "Brass Section", "Synth Brass 1", "Synth Brass 2",
    "Soprano Sax", "Alto Sax", "Tenor Sax", "Baritone Sax", "Oboe", "English Horn", "Bassoon", "Clarinet",
    "Piccolo", "Flute", "Recorder", "Pan Flute", "Blown Bottle", "Shakuhachi", "Whistle", "Ocarina",
    "Lead 1 (square)", "Lead 2 (sawtooth)", "Lead 3 (calliope)", "Lead 4 (chiff)", "Lead 5 (charang)", "Lead 6 (voice)", "Lead 7 (fifths)", "Lead 8 (bass + lead)",
    "Pad 1 (new age)", "Pad 2 (warm)", "Pad 3 (polysynth)", "Pad 4 (choir)", "Pad 5 (bowed)", "Pad 6 (metallic)", "Pad 7 (halo)", "Pad 8 (sweep)",
    "FX 1 (rain)", "FX 2 (soundtrack)", "FX 3 (crystal)", "FX 4 (atmosphere)", "FX 5 (brightness)", "FX 6 (goblins)", "FX 7 (echoes)", "FX 8 (sci-fi)",
    "Sitar", "Banjo", "Shamisen", "Koto", "Kalimba", "Bagpipe", "Fiddle", "Shanai",
    "Tinkle Bell", "Agogo", "Steel Drums", "Woodblock", "Taiko Drum", "Melodic Tom", "Synth Drum", "Reverse Cymbal",
    "Guitar Fret Noise", "Breath Noise", "Seashore", "Bird Tweet", "Telephone Ring", "Helicopter", "Applause", "Gunshot",
];

/// What a part does in the arrangement; it picks the instrument, the bus
/// and the sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Drums,
    Bass,
    Keys,
    Guitar,
    Pads,
    Strings,
    Brass,
    Lead,
    Fx,
}

impl Role {
    pub const ALL: [Role; 9] = [Role::Drums, Role::Bass, Role::Keys, Role::Guitar, Role::Pads, Role::Strings, Role::Brass, Role::Lead, Role::Fx];

    pub fn name(self) -> &'static str {
        match self {
            Role::Drums => "drums",
            Role::Bass => "bass",
            Role::Keys => "keys",
            Role::Guitar => "guitar",
            Role::Pads => "pads",
            Role::Strings => "strings",
            Role::Brass => "brass",
            Role::Lead => "lead",
            Role::Fx => "fx",
        }
    }

    pub fn parse(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.name() == name)
    }
}

/// The role a GM program plays (the drum channel is [`Role::Drums`]
/// whatever its program).
pub fn role(program: u8) -> Role {
    match program {
        0..=23 => Role::Keys,
        24..=31 => Role::Guitar,
        32..=39 => Role::Bass,
        40..=51 => Role::Strings,
        52..=54 => Role::Pads,
        55 => Role::Fx,
        56..=63 => Role::Brass,
        64..=87 => Role::Lead,
        88..=95 => Role::Pads,
        96..=103 => Role::Fx,
        104..=111 => Role::Guitar,
        112..=119 => Role::Keys,
        _ => Role::Fx,
    }
}

/// How a cover plays a part: the engine's production preset for a synth
/// role, the GM SoundFont for acoustic instruments, a kit for the drums.
#[derive(Clone, Debug, PartialEq)]
pub enum Voice {
    /// `Synth{preset: @name}` (no node-level cutoff: it would replace the
    /// patch's own filter sweep).
    Preset(&'static str),
    /// `Instrument{kind: @soundfont program: n}`.
    Gm(u8),
    /// `Drums{kit: @name}`.
    Kit(&'static str),
    /// `Instrument{kind: @drums}`: the modelled acoustic kit.
    ModelledKit,
    /// `Instrument{kind: @soundfont bank: 128 program: k}`: a GM drum kit
    /// (0 standard, 8 room, 16 power, 24 electronic, 25 analog machine, 32
    /// jazz, 40 brush, 48 orchestra).
    GmKit(u8),
    /// A Score's own instrument, described (a part read back from a
    /// Score); written as the keys preset when it becomes a Score again.
    Other(String),
    /// An instrument node written as it is (`Sampler{src: asset("…", "1")
    /// root: 60}`, a chip patch): a tracker's sample, a SID voice.
    Node(String),
}

/// The GM drum kit a style wants (bank 128): the analog drum machine (25)
/// for hip-hop and electro, the electronic kit (24) for electronic and
/// dance music and synth-pop, power (16) for rock, jazz (32), else the
/// standard kit (0).
pub fn kit_for(genre: &str, tags: &[(String, f32)]) -> Voice {
    // Whole words: "electro" is not "electronic".
    let has = |words: &[&str]| words.iter().any(|w| genre.split(',').any(|g| g.trim_start_matches('~') == *w) || tags.iter().take(10).any(|(t, _)| t == w));
    Voice::GmKit(if has(&["hiphop", "rap", "electro", "trap", "crunk", "oldschoolhiphop"]) {
        25
    } else if has(&["electronic", "dance", "synthpop", "techno", "house", "trance", "edm", "eurodance", "electropop", "hinrg", "disco", "newwave"]) {
        24
    } else if has(&["rock", "metal", "punk", "hardrock", "grunge"]) {
        16
    } else if has(&["jazz", "swing", "bebop"]) {
        32
    } else {
        0
    })
}

/// The voice a pitched part plays on: its own GM program (the SoundFont
/// sounds best; the arranger may choose otherwise per part).
pub fn voice(program: u8) -> Voice {
    Voice::Gm(program)
}

/// The engine's production preset for a synth role, for a cover that asks
/// for them (`--presets`): synth basses, leads, pads, e-piano, clav, organ,
/// synth strings and brass; none for acoustic programs.
pub fn preset(program: u8) -> Option<Voice> {
    Some(Voice::Preset(match program {
        4 | 5 => "epiano",
        7 => "clav",
        16..=18 => "organ",
        38 | 39 => "fm_bass",
        50 | 51 => "strings",
        61..=63 => "brass",
        80 => "square_lead",
        81..=87 => "saw_lead",
        88 | 89 => "warm_pad",
        90 => "stab",
        92..=94 => "glass_pad",
        95 => "pad",
        _ => return None,
    }))
}

impl Voice {
    /// The instrument's node, as the score writes it.
    pub fn node(&self) -> String {
        match self {
            Voice::Preset(name) => format!("Synth{{preset: @{name}}}"),
            Voice::Gm(program) => format!("Instrument{{kind: @soundfont program: {program}}}"),
            Voice::Kit(kit) => format!("Drums{{kit: @{kit}}}"),
            Voice::ModelledKit => "Instrument{kind: @drums}".into(),
            Voice::GmKit(program) => format!("Instrument{{kind: @soundfont bank: 128 program: {program}}}"),
            Voice::Other(_) => "Synth{preset: @keys}".into(),
            Voice::Node(node) => node.clone(),
        }
    }

    /// Whether it is a native instrument (it takes `@pedal` and program
    /// changes; a synth preset's notes hold their own length instead).
    pub fn native(&self) -> bool {
        matches!(self, Voice::Gm(_) | Voice::ModelledKit | Voice::GmKit(_))
    }

    pub fn about(&self) -> String {
        match self {
            Voice::Preset(name) => format!("the {name} preset"),
            Voice::Gm(program) => format!("GM {}", PROGRAMS[*program as usize & 127]),
            Voice::Kit(kit) => format!("the {kit} kit"),
            Voice::ModelledKit => "the modelled acoustic kit".into(),
            Voice::GmKit(program) => format!("the GM {} kit", gm_kit_name(*program)),
            Voice::Other(about) => about.clone(),
            Voice::Node(node) => node.split('{').next().map_or("an instrument".into(), |kind| format!("its own {kind}")),
        }
    }
}

/// A GM percussion note by the name the score writes (`@kick`) when the
/// name means exactly that note; other pieces are written as numbers.
pub fn drum_name(note: u8) -> Option<&'static str> {
    Some(match note {
        36 => "kick",
        37 => "rim",
        38 => "snare",
        39 => "clap",
        42 => "hat",
        44 => "pedalhat",
        46 => "ohat",
        43 => "floortom",
        45 => "tom3",
        47 => "tom2",
        50 => "tom1",
        49 => "crash",
        51 => "ride",
        53 => "bell",
        56 => "cowbell",
        63 => "conga",
        70 => "shaker",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_voices() {
        assert_eq!(PROGRAMS[33], "Electric Bass (finger)");
        assert_eq!(role(0), Role::Keys);
        assert_eq!(role(38), Role::Bass);
        assert_eq!(role(81), Role::Lead);
        assert_eq!(role(89), Role::Pads);
        assert_eq!(voice(38), Voice::Gm(38));
        assert_eq!(preset(38), Some(Voice::Preset("fm_bass")));
        assert_eq!(preset(0), None);
        assert_eq!(voice(0), Voice::Gm(0));
        assert_eq!(voice(25), Voice::Gm(25));
        assert!(Voice::Gm(48).native() && !Voice::Preset("pad").native());
        assert_eq!(kit_for("electronic", &[]), Voice::GmKit(24));
        assert_eq!(kit_for("rock", &[("synthpop".into(), 1.0)]), Voice::GmKit(24));
        assert_eq!(kit_for("rock", &[]), Voice::GmKit(16));
        assert_eq!(kit_for("electro", &[]), Voice::GmKit(25));
        assert_eq!(Voice::GmKit(25).node(), "Instrument{kind: @soundfont bank: 128 program: 25}");
        for r in Role::ALL {
            assert_eq!(Role::parse(r.name()), Some(r));
        }
        // Every drum name is one the score reads as that very GM note.
        for note in 35..=81u8 {
            if let Some(name) = drum_name(note) {
                let src = format!("use mod.score.*\nlet clock = Clock{{bpm: 120}}\nlet k = Instrument{{kind: @drums}}\nlet l = Loop{{play: k notes: [[0 @{name} 0.25 0.8]]}}\nScore{{clock k l}}");
                let loaded = makepad_score_live::model::load(&src).unwrap_or_else(|d| panic!("{name}: {d:?}"));
                assert_eq!(loaded.doc.loops[0].notes.as_ref().unwrap()[0].note, note as f32, "{name}");
            }
        }
    }
}

/// A GM drum kit's name by its program.
pub fn gm_kit_name(program: u8) -> &'static str {
    match program {
        8 => "room",
        16 => "power",
        24 => "electronic",
        25 => "analog drum-machine",
        32 => "jazz",
        40 => "brush",
        48 => "orchestra",
        _ => "standard",
    }
}
