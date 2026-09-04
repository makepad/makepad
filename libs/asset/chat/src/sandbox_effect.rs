//! Effects of Sandbox tools, shared by clients and offline replay scorers.
//! Unknown tools are not reads: callers must handle `None` explicitly.
use crate::tools::{canonicalize_tool_name, ContentToolCall};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxEffect {
    Read,
    WorldMutation,
    /// Generates content without directly editing the running world.
    ContentMutation,
}

pub fn sandbox_effect(name: &str) -> Option<SandboxEffect> {
    use SandboxEffect::*;
    match canonicalize_tool_name(name).as_str() {
        "assets.query" | "assets.schema" | "world.list" | "world.get_source"
        | "world.get_plan" | "world.api" | "model.fetch" => Some(Read),
        "world.place" | "world.remove" | "world.move" | "world.spawn"
        | "world.add_addon" | "world.tune" | "world.set_source" | "world.set_plan"
        | "world.new_level" | "world.set_player_model" => Some(WorldMutation),
        "content.generate" | "model.build" => Some(ContentMutation),
        _ => None,
    }
}

impl ContentToolCall {
    /// Includes calls wrapped in a named world subasset (`name()` unwraps it).
    pub fn sandbox_effect(&self) -> Option<SandboxEffect> {
        sandbox_effect(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{encode_args, sandbox_definitions};
    use makepad_asset_client::json::{self, Value};

    #[test]
    fn every_definition_and_wire_spelling_has_an_effect() {
        let defs = sandbox_definitions();
        assert_eq!(defs.len(), 19, "classify new Sandbox definitions explicitly");
        for def in defs {
            let effect = sandbox_effect(def.name).expect(def.name);
            assert_eq!(sandbox_effect(def.api_name), Some(effect), "{}", def.name);
            assert_eq!(sandbox_effect(&def.name.replace('.', "_")), Some(effect));
        }
        // `world.make` is not a tool in this checkout. Do not invent an
        // alias/router: a future definition must also acquire an effect.
        assert_eq!(sandbox_effect("world.make"), None);
        assert!(ContentToolCall::parse("world.make", &json::obj(vec![])).is_err());
        assert_eq!(sandbox_effect("world.future"), None);
        assert_eq!(sandbox_effect("model.fetch"), Some(SandboxEffect::Read));
    }

    #[test]
    fn enum_names_and_real_parser_agree_on_world_mutations() {
        use ContentToolCall::*;
        let calls = vec![
            WorldList, WorldGetSource, WorldGetPlan,
            WorldSetPlan { plan: json::obj(vec![("v", Value::Int(1))]), revision: 0, note: None },
            WorldSetSource { source: "game.box({})".into(), note: None },
            WorldNewLevel { title: "Test".into(), source: "game.box({})".into(), note: None },
            WorldRemove { ids: vec![1], tag: None },
            WorldMove { id: 1, pos: Some([0.0; 3]), yaw_deg: None, scale: None },
            WorldSetPlayerModel { model: "kenney/mini-characters/character-male-a".into() },
            WorldSpawn { model: "kenney/nature-kit/tree".into(), pos: None, form: None, scale: None, color: None, hue: None, tag: None },
            WorldTune { time: Some(12.0), car_speed: None },
            WorldAddAddon { name: "box".into(), src: "game.box({})".into() },
        ];
        for (i, call) in calls.into_iter().enumerate() {
            let expected = if i < 3 { SandboxEffect::Read } else { SandboxEffect::WorldMutation };
            let parsed = ContentToolCall::parse(call.name(), &encode_args(&call)).expect(call.name());
            assert_eq!(parsed.sandbox_effect(), Some(expected), "{}", call.name());
        }
        let placed = ContentToolCall::parse("world.place", &json::parse(
            br#"{"items":[{"model":"kenney/nature-kit/tree","pos":[0,0,0]}]}"#,
        ).unwrap()).unwrap();
        assert_eq!(placed.name(), "world.place");
        assert_eq!(placed.sandbox_effect(), Some(SandboxEffect::WorldMutation));
        let wrapped = ContentToolCall::parse("world.set_plan", &json::parse(
            br#"{"plan":{"v":1},"revision":0,"sub":"room"}"#,
        ).unwrap()).unwrap();
        assert_eq!(wrapped.sandbox_effect(), Some(SandboxEffect::WorldMutation));
    }
}
