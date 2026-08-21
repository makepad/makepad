//! The teaching context: what a fresh chat session's model is told about
//! this system, assembled SERVER-side at every send.
//!
//! Three layers, per the design:
//! 1. **Base** (all apps): the architecture in a few lines, alias
//!    conventions, the catalog query workflow. One text, versioned here.
//! 2. **Per-app**: selected by the connecting app's declared profile at
//!    session create. The game profile carries the splash level-authoring
//!    guide (written to double, verbatim, as the brief handed to an
//!    external drafting model when that phase-2 delegation is wired).
//! 3. **Dynamic**: a compact live snapshot the broker generates per send —
//!    schema summary from `sqlite_master`, headline asset counts — via
//!    [`crate::catalog_sql`].
//!
//! The exact tool contracts are NOT duplicated here: the session engine
//! renders them live from the advertised `ToolDef`s, so the docs can never
//! drift from the parser.
//!
//! Budget: the whole assembled stack must stay small enough that real work
//! fits in the fleet model's context — [`MAX_CONTEXT_BYTES`] is asserted in
//! tests against base+app+typical dynamic.

/// ~8k tokens at ~4 bytes/token: the ceiling for base + app + dynamic.
pub const MAX_CONTEXT_BYTES: usize = 32_000;

pub const BASE: &str = include_str!("../context/base.md");
pub const GAME: &str = include_str!("../context/game.md");
pub const VJ: &str = include_str!("../context/vj.md");
pub const GEN: &str = include_str!("../context/gen.md");

/// Which app-flavored context (and client-tool surface) a session gets.
/// Declared by the connecting app at session create; unknown slugs are
/// refused at the route, never silently defaulted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientProfile {
    /// Librarian framing only (anything undeclared).
    General,
    /// The game sandbox: splash authoring + client-executed world tools.
    Game,
    /// The VJ performance app.
    Vj,
    /// The asset UI's Create pane: content generation, with the fleet
    /// generate/defaults tools executed by that app's own pipeline.
    Gen,
}

impl ClientProfile {
    pub fn from_slug(s: &str) -> Option<ClientProfile> {
        match s {
            "general" => Some(ClientProfile::General),
            "game" => Some(ClientProfile::Game),
            "vj" => Some(ClientProfile::Vj),
            "gen" => Some(ClientProfile::Gen),
            _ => None,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            ClientProfile::General => "general",
            ClientProfile::Game => "game",
            ClientProfile::Vj => "vj",
            ClientProfile::Gen => "gen",
        }
    }

    fn app_layer(self) -> &'static str {
        match self {
            ClientProfile::General => "",
            ClientProfile::Game => GAME,
            ClientProfile::Vj => VJ,
            ClientProfile::Gen => GEN,
        }
    }

    /// Which calls the CONNECTED APP executes: the broker parks the turn on
    /// the tool-result route instead of running the tool itself.
    ///
    /// Two profiles declare tools of their own. The game's `world.*` edit
    /// the running level, which only that process has. The asset UI's
    /// generate/defaults/fleet tools drive the app's own fleet pipeline —
    /// the server-side dispatcher has always said so in its refusal text
    /// ("executed by the AI Content app"); this is the seam that actually
    /// sends them there.
    pub fn client_executes(self, call: &crate::tools::ContentToolCall) -> bool {
        use crate::tools::ContentToolCall as C;
        match self {
            ClientProfile::Game => matches!(
                call,
                C::WorldPlace { .. }
                    | C::WorldRemove { .. }
                    | C::WorldMove { .. }
                    | C::WorldList
                    | C::WorldGetSource
                    | C::WorldSetSource { .. }
                    | C::WorldSetPlayerModel { .. }
                    | C::WorldSpawn { .. }
                    | C::WorldTune { .. }
                    | C::WorldAddAddon { .. }
            ),
            ClientProfile::Gen => matches!(
                call,
                C::ImageGenerate { .. }
                    | C::VideoGenerate { .. }
                    | C::AudioGenerate { .. }
                    | C::SpeechGenerate { .. }
                    | C::MusicGenerate { .. }
                    | C::MeshGenerate { .. }
                    | C::WorldGenerate { .. }
                    | C::CharacterGenerate { .. }
                    | C::DefaultsGet
                    | C::DefaultsSet { .. }
                    | C::FleetIntrospect { .. }
            ),
            ClientProfile::General | ClientProfile::Vj => false,
        }
    }
}

/// Base + app + dynamic, in that order. `dynamic` is the broker's live
/// snapshot (schema, counts, delegate capabilities); it is bounded by the
/// caller.
pub fn assemble(profile: ClientProfile, dynamic: &str) -> String {
    let app = profile.app_layer();
    let mut out = String::with_capacity(BASE.len() + app.len() + dynamic.len() + 8);
    out.push_str(BASE);
    if !app.is_empty() {
        out.push('\n');
        out.push_str(app);
    }
    if !dynamic.is_empty() {
        out.push('\n');
        out.push_str(dynamic);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_assembled_context_fits_the_token_budget() {
        // A generous stand-in for the dynamic layer: a 40-table schema
        // summary plus counts is ~4 KB in practice.
        let dynamic = "x".repeat(6_000);
        for profile in [
            ClientProfile::General,
            ClientProfile::Game,
            ClientProfile::Vj,
            ClientProfile::Gen,
        ] {
            let text = assemble(profile, &dynamic);
            assert!(
                text.len() < MAX_CONTEXT_BYTES,
                "{:?} context is {} bytes — over the {} budget",
                profile,
                text.len(),
                MAX_CONTEXT_BYTES
            );
        }
    }

    #[test]
    fn profiles_roundtrip_and_unknown_is_refused() {
        for p in [
            ClientProfile::General,
            ClientProfile::Game,
            ClientProfile::Vj,
            ClientProfile::Gen,
        ] {
            assert_eq!(ClientProfile::from_slug(p.slug()), Some(p));
        }
        assert_eq!(ClientProfile::from_slug("root"), None);
        assert_eq!(ClientProfile::from_slug(""), None);
    }

    /// Each profile parks exactly its own app's tools on the client, and
    /// nobody else's: a general session that waited for an answer no client
    /// is going to send would stall for the whole park timeout.
    #[test]
    fn only_the_declaring_profile_parks_its_tools_on_the_app() {
        use crate::tools::ContentToolCall as C;
        let place = C::WorldPlace { items: Vec::new() };
        let generate = C::ImageGenerate {
            prompt: "a rusty trawler".into(),
            then: None,
            model: None,
            width: None,
            height: None,
            steps: None,
        };
        let search = C::AssetSearch { query: "trawler".into(), limit: 8 };

        assert!(ClientProfile::Game.client_executes(&place));
        assert!(!ClientProfile::Game.client_executes(&generate));
        assert!(ClientProfile::Gen.client_executes(&generate));
        assert!(ClientProfile::Gen.client_executes(&C::DefaultsGet));
        assert!(ClientProfile::Gen.client_executes(&C::FleetIntrospect { domain: None }));
        assert!(!ClientProfile::Gen.client_executes(&place));
        for profile in [ClientProfile::General, ClientProfile::Vj] {
            assert!(!profile.client_executes(&generate));
            assert!(!profile.client_executes(&place));
        }
        for profile in [
            ClientProfile::General,
            ClientProfile::Game,
            ClientProfile::Vj,
            ClientProfile::Gen,
        ] {
            assert!(!profile.client_executes(&search), "{profile:?} runs search on the server");
        }
    }

    #[test]
    fn the_gen_layer_teaches_the_generate_surface() {
        for needle in
            ["image.generate", "defaults.set", "fleet.introspect", "Create page"]
        {
            assert!(GEN.contains(needle), "gen context lost: {needle}");
        }
    }

    #[test]
    fn the_game_layer_teaches_the_load_bearing_rules() {
        for needle in [
            "world.get_source",
            "world.set_source",
            "vec3",
            "smooth: true",
            "canon_alias",
            "RADIANS",
        ] {
            assert!(GAME.contains(needle), "game context lost: {needle}");
        }
        assert!(BASE.contains("assets.schema"));
        assert!(BASE.contains("live=1"));
    }
}
