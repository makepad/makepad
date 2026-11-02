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
}
