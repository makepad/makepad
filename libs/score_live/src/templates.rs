//! The bundled scores: starting points for people and the AI, and the
//! examples the composer guide points at.

pub const TEMPLATES: [(&str, &str); 6] = [
    ("ambient", include_str!("../examples/ambient.splash")),
    ("techno", include_str!("../examples/techno.splash")),
    ("chiptune", include_str!("../examples/chiptune.splash")),
    ("generative", include_str!("../examples/generative.splash")),
    ("vj_sync", include_str!("../examples/vj_sync.splash")),
    ("blank", include_str!("../examples/blank.splash")),
];

pub fn template(name: &str) -> Option<&'static str> {
    TEMPLATES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

#[cfg(test)]
mod tests {
    use crate::compose::Composer;
    use crate::render::{render, RenderOpts};

    #[test]
    fn every_template_loads_plays_and_raises_nothing() {
        for (name, source) in super::TEMPLATES {
            let loaded = crate::model::load(source).unwrap_or_else(|d| panic!("{name}: {d:?}"));
            let mut composer = Composer::new(loaded);
            let mut events = Vec::new();
            composer.pump(64.0, &mut events);
            assert!(composer.errors().is_empty(), "{name}: {:?}", composer.errors());
            assert!(!events.is_empty(), "{name}: no events");
            let r = render(source, &RenderOpts { seconds: Some(6.0), ..Default::default() }, &crate::NoSamples).unwrap();
            let peak = r.left.iter().fold(0.0f32, |a, x| a.max(x.abs()));
            assert!(peak > 0.02, "{name}: silent ({peak})");
        }
    }

    /// The example pieces (examples/music): each loads, runs every loop
    /// without an error and sounds, all the way through its first minute.
    #[test]
    fn every_example_piece_loads_plays_and_raises_nothing() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/music");
        let mut pieces: Vec<_> = std::fs::read_dir(&dir).map(|d| d.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "splash")).collect()).unwrap_or_default();
        pieces.sort();
        assert!(pieces.len() >= 12, "the example pieces are missing from {}", dir.display());
        for path in pieces {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let source = std::fs::read_to_string(&path).unwrap();
            let loaded = crate::model::load(&source).unwrap_or_else(|d| panic!("{name}: {d:?}"));
            // A General MIDI piece needs the GM SoundFont, which a machine
            // may not have installed (renders refuse without it).
            if loaded.doc.plays_gm_font() && !crate::gm_font::installed() {
                eprintln!("{name}: skipped, the General MIDI SoundFont is not installed");
                continue;
            }
            let mut composer = Composer::new(loaded);
            let mut events = Vec::new();
            composer.pump(256.0, &mut events);
            assert!(composer.errors().is_empty(), "{name}: {:?}", composer.errors());
            // The piece as written (its arrangement and tail), at most a minute.
            let doc = crate::model::ScoreDoc::load(&source).unwrap();
            let seconds = crate::render::natural_length(&doc).min(60.0);
            let r = render(&source, &RenderOpts { seconds: Some(seconds), ..Default::default() }, &crate::NoSamples).unwrap();
            assert!(r.left.iter().chain(&r.right).all(|x| x.is_finite()), "{name}: not finite");
            // Every ten seconds has sound in it (nothing goes silent) until
            // the tail.
            let music = ((seconds - 2.0).max(1.0) * 48_000.0) as usize;
            for (k, window) in r.left[..music.min(r.left.len())].chunks(480_000).enumerate() {
                let peak = window.iter().fold(0.0f32, |a, x| a.max(x.abs()));
                assert!(peak > 0.01, "{name}: silent from {}s ({peak})", k * 10);
            }
        }
    }
}
