//! Authoring scope is a user preference, independent of provider and permissions.
// Shared codec: public chat tests must not depend on the private Sandbox checkout.
use makepad_asset_client::{ChatProviderKind, ChatProviderLocality};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChatPrefs {
    pub provider: ChatProviderKind,
    pub local_only: bool,
    pub author_policy: AuthorPolicy,
}

impl Default for ChatPrefs {
    fn default() -> Self {
        // Local by default: a fresh install must not reach a vendor before
        // the user has said it may.
        ChatPrefs { provider: ChatProviderKind::FleetQwen, local_only: true, author_policy: AuthorPolicy::Auto }
    }
}

impl ChatPrefs {
    /// Parse the preference file (including old two-line files). Anything unreadable falls back to the
    /// default rather than failing — a corrupt pref must not cost the user
    /// their chat.
    pub fn parse(text: &str) -> ChatPrefs {
        let mut prefs = ChatPrefs::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "provider" => {
                    if let Some(kind) = ChatProviderKind::parse(value.trim()) {
                        prefs.provider = kind;
                    }
                }
                "author_policy" => {
                    if let Some(policy) = AuthorPolicy::parse(value.trim()) { prefs.author_policy = policy; }
                }
                "local_only" => match value.trim() {
                    "true" => prefs.local_only = true,
                    "false" => prefs.local_only = false,
                    _ => {}
                },
                _ => {}
            }
        }
        // A cloud provider UNDER the lock is not a state this app can be
        // in; normalising here means no caller has to remember the rule.
        if prefs.local_only && prefs.provider.default_locality() == ChatProviderLocality::Cloud {
            prefs.provider = ChatProviderKind::FleetQwen;
        }
        prefs
    }

    pub fn render(&self) -> String {
        format!(
            "provider={}\nlocal_only={}\nauthor_policy={}\n",
            self.provider.as_str(),
            self.local_only,
            self.author_policy.slug()
        )
    }

}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthorPolicy {
    Guided,
    Expert,
    #[default]
    Auto,
}

impl AuthorPolicy {
    pub const ALL: [Self; 3] = [Self::Guided, Self::Expert, Self::Auto];

    pub fn slug(self) -> &'static str {
        match self { Self::Guided => "guided", Self::Expert => "expert", Self::Auto => "auto" }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|policy| policy.slug() == value)
    }

    pub fn exact_source(self) -> bool { self == Self::Expert }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthorScope {
    Map,
    #[default]
    Staged,
}

/// Narrow only positively identified, pure map work. Uncertain and composite
/// requests retain tools. History is scoped by the caller to the current game.
pub fn scope(text: &str, previous: Option<&str>) -> AuthorScope {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty()).collect();
    // Test the original spelling FIRST: woods is not wood, class is not cla.
    let has = |key: &str| words.iter().any(|w| *w == key || w.strip_suffix('s') == Some(key));
    let content = ["model", "rig", "dragon", "character", "spawn", "player", "car", "race",
        "racetrack", "banked", "script", "code", "custom", "exact", "pinned", "floorless",
        "controller", "gameplay", "sound", "music", "prop", "sword", "faster", "slower",
        "animate", "animation", "assemble", "csg", "mesh", "skin", "weight"];
    if content.iter().any(|key| has(key)) { return AuthorScope::Staged; }
    // A pronoun may refer to a custom object even when this turn names its
    // village. Keep its creation/edit tools instead of guessing a map target.
    if ["it", "them", "that", "those", "same"].iter().any(|key| has(key)) {
        if let Some(previous) = previous {
            return scope(previous, None);
        }
        return AuthorScope::Staged;
    }
    let map = ["map", "plan", "terrain", "landscape", "river", "lake", "canal", "road",
        "highway", "street", "railway", "rail", "monorail", "bridge", "town", "village",
        "city", "mountain", "hill", "valley", "crater", "ridge", "plateau", "forest",
        "woods", "desert", "alpine", "snowy", "tundra", "biome", "airfield", "runway",
        "helipad", "coaster", "landform", "island"];
    // Do not let an unfamiliar noun ("dragon", "track", "quest", …) disappear
    // just because the request also contains "village". A narrow turn must be
    // positively understood end to end; unknown vocabulary keeps discovery.
    let grammar = ["a", "an", "the", "and", "or", "with", "without", "in", "on", "at", "of",
        "to", "from", "by", "for", "me", "my", "you", "can", "could", "please", "make",
        "build", "create", "add", "remove", "delete", "change", "move", "widen", "narrow",
        "more", "less", "some", "no", "world", "west", "east", "north", "south", "between",
        "near", "beside", "through", "across", "around", "wide", "small", "large", "big",
        "tall", "taller", "high", "low", "flat", "steep", "deep", "shallow", "snow",
        "green", "sandy", "rocky", "wooded", "dense", "two", "three", "four", "five"];
    let understood = words.iter().all(|word| {
        grammar.contains(word) || map.iter().any(|key| *word == *key || word.strip_suffix('s') == Some(key))
            || word.parse::<f64>().is_ok()
    });
    if understood && map.iter().any(|key| has(key)) { AuthorScope::Map } else { AuthorScope::Staged }
}

pub fn doctrine(policy: AuthorPolicy, scope: AuthorScope) -> String {
    let active = if policy == AuthorPolicy::Expert { "expert" } else { "guided" };
    let mut out = format!("Authoring policy: {} (active: {active}; scope: {scope:?}). ", policy.slug());
    out.push_str("NEVER ASK BEFORE BUILDING when the request is clear. Read the target before editing; preserve both generated map code and authored scripts/models. Publication returns an alias; placement is a separate action. For an explicit original-modeling request (for example, model me an old car), author a new editable asset with model.open/apply/publish; discover model.workflow first. Build at intended metre dimensions; construction is full scale unless the user explicitly requests a handheld/XR miniature (model.open preview_scale affects display only). Catalog lookup is for requested reuse, not a substitute for original modeling. Await accepted document jobs with model.jobs before dependent edits or placement. If the user asks for image choices before modeling, call model.concepts once with up to three prompts (flux1-schnell), then wait for their selection. The chosen image is attached to their next message; model from it and use model.texture reference:selected when an image-edit model is appropriate. Before publication, model.render the final head. Inspect rest AND moving views for missing faces, wheel/body clipping, joint collapse and clothing penetration. Preserve the intended silhouette; never create giant fenders just to pass a diagnostic pose. Author wheel visual steering/travel limits separately from physics when appropriate; review uses those same limits. Fix actual clearances, pivots/weights or corrective poses without silently changing handling. Use explicit motion samples for controller limits and character extremes. Re-render repairs; report gaps after three failed cycles. Generator helpers accelerate work within their declared capabilities; a plan cannot create a character, rig, controller or race. ");
    if policy == AuthorPolicy::Expert {
        out.push_str("Use world.api to discover exact live signatures. game.ui accepts inline HUD shaders; game.material defines procedural model surfaces with live vec4 parameters. Discover their supported targets and limits before using them. ");
        out.push_str("Expert policy supersedes the beginner placement/modeling restrictions in the game guide, not permission, budget or transaction safeguards. Use world.get_source and model.fetch to discover current source, world.add_addon for scoped scripts, model.open/apply for editable geometry, materials, lights and rigs, model.build for requested legacy CSG, world.spawn for body/controller assembly, and world.set_source for precise edits. Algorithmic helpers (game.village, game.traintrack, world.plan) remain available when useful. Exact, floorless and pinned construction is deliberate: no automatic floor or building-spacing rewrite. Discover model.rig for actual skeleton, weights, IK and animation controls; rigid pivot animation remains distinct from skinning. ");
    } else if scope == AuthorScope::Map {
        out.push_str("For this pure map task use world.get_plan then world.set_plan with the returned revision and schema. Map generators ARE ONE CALL; use their diagnostics to correct the plan. ");
    } else {
        out.push_str("Stage mixed work: inspect the map, use plan helpers for supported terrain/layout, then create content and add gameplay via scoped tools. Read source before an edit and retain unrelated lines. Use complete catalog assets for reuse and automatic helpers for supported layouts. Original modeling uses model.open/apply/publish; use a scoped addon for gameplay. ");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composite_and_follow_up_routes_keep_creation() {
        for text in ["model and rig a dragon for this village", "a banked racetrack in an alpine valley",
            "make the cars faster and add woods", "custom script for the ridge", "spawn a car on the road",
            "make a village with a train", "an RTS map with armies", "add units to the map",
            "soldiers in the village", "zombies in the woods", "enemies on the road",
            "a village with a quest", "a mountain with a teleporter", "model me an old car"] {
            assert_eq!(scope(text, None), AuthorScope::Staged, "{text}");
        }
        assert_eq!(scope("add woods", None), AuthorScope::Map);
        assert_eq!(scope("make it taller", Some("model a dragon for the village")), AuthorScope::Staged);
        assert_eq!(scope("make it taller", Some("add a mountain")), AuthorScope::Map);
        assert_eq!(scope("move it beside the village", None), AuthorScope::Staged);
        assert_eq!(AuthorPolicy::parse("vendor-smart"), None);
    }
}
