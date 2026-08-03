//! Agent-facing surface: a backend-agnostic tool the model calls to SEARCH the
//! library, plus a tiny prompt blurb telling it the library exists.
//!
//! The agent never receives the catalogue. Hundreds of entries do not fit in a
//! prompt, every result token is paid for, and the library grows with every
//! pack — so the contract is "call `find_model`, never guess an id".
//!
//! Everything here is plain data so any backend (Claude Code, direct API,
//! OpenAI-compatible) can expose it; this module deliberately does not depend
//! on `makepad_ai`.

use crate::{AssetIndex, AssetKind, Filters, Source};

/// A tool parameter, in the subset of JSON Schema every provider understands.
pub struct ToolParam {
    pub name: &'static str,
    pub ty: &'static str,
    pub description: &'static str,
    pub required: bool,
}

/// A provider-neutral tool description. Render it into whatever shape the
/// backend wants.
pub struct ToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [ToolParam],
}

pub const FIND_MODEL: ToolDescriptor = ToolDescriptor {
    name: "find_model",
    description: "Search the stock asset library — 3D models, sound effects and music — by \
                  description, and get back ids you can use. Use plain language: what the thing \
                  is, or what it is FOR (\"red truck\", \"something to hide behind\", \"trees \
                  for a forest\", \"sound when you crash\", \"happy win music\"). Always call \
                  this before using an asset id; never invent one.",
    params: &[
        ToolParam {
            name: "query",
            ty: "string",
            description: "What you are looking for, in plain language.",
            required: true,
        },
        ToolParam {
            name: "kind",
            ty: "string",
            description: "Optional: \"model\", \"sound\" or \"music\".",
            required: false,
        },
        ToolParam {
            name: "category",
            ty: "string",
            description: "Optional category filter, e.g. \"vehicle\", \"nature/tree\", \
                          \"character/enemy\", \"sound/impact\".",
            required: false,
        },
        ToolParam {
            name: "rigged_only",
            ty: "boolean",
            description: "Models only: require a skeleton (can be animated).",
            required: false,
        },
        ToolParam {
            name: "max_results",
            ty: "integer",
            description: "How many results to return (default 5, max 20).",
            required: false,
        },
    ],
};

/// Parameters for one `find_model` call. The app fills these from whatever the
/// backend handed back; all optional except `query`.
#[derive(Default, Debug)]
pub struct FindParams {
    pub query: String,
    pub kind: Option<AssetKind>,
    pub category: Option<String>,
    pub rigged_only: bool,
    pub max_results: Option<usize>,
}

impl FindParams {
    pub fn new(query: &str) -> Self {
        FindParams { query: query.to_string(), ..Default::default() }
    }
    /// Parse the `kind` argument a model would send. Unknown values mean "no
    /// filter" rather than an error — a stray word should not fail the call.
    pub fn with_kind_str(mut self, kind: &str) -> Self {
        self.kind = match kind.to_lowercase().as_str() {
            "model" => Some(AssetKind::Model),
            "sound" | "sfx" => Some(AssetKind::Sound),
            "music" => Some(AssetKind::Music),
            _ => None,
        };
        self
    }
}

/// One compact result row. Deliberately narrow: id, name, category, rigged and
/// size are all the model needs to choose. Paths and index internals are not
/// the agent's business and would cost tokens for nothing.
pub struct FindResult {
    pub id: String,
    pub name: String,
    pub kind: AssetKind,
    pub category: String,
    pub rigged: bool,
    pub size: Option<[f32; 3]>,
    /// Audio only: a looping sound is held, not fired.
    pub loops: bool,
    /// False when the repo cannot decode this asset yet (Kenney audio is Ogg
    /// Vorbis and there is no vorbis decoder in the tree).
    pub playable: bool,
}

/// Run a `find_model` call.
pub fn execute(index: &AssetIndex, params: &FindParams) -> Vec<FindResult> {
    let limit = params.max_results.unwrap_or(5).clamp(1, 20);
    let filters = Filters {
        rigged_only: params.rigged_only,
        category: params.category.clone(),
        source: None,
        max_extent: None,
        kind: params.kind,
    };
    index
        .find_filtered(&params.query, &filters)
        .into_iter()
        .take(limit)
        .map(|h| FindResult {
            id: h.entry.id.clone(),
            name: h.entry.name.clone(),
            kind: h.entry.kind,
            category: h.entry.categories.first().cloned().unwrap_or_default(),
            rigged: h.entry.rigged,
            size: h.entry.size,
            loops: h.entry.loops,
            playable: h.entry.decodable,
        })
        .collect()
}

/// Render results as compact JSON for handing back to a model. One line per
/// result, no pretty-printing — this is paid-for context.
pub fn results_to_json(results: &[FindResult]) -> String {
    let mut s = String::from("[");
    for (i, r) in results.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"id\":\"{}\",\"name\":\"{}\",\"kind\":\"{}\",\"category\":\"{}\"",
            r.id,
            r.name,
            r.kind.as_str(),
            r.category
        ));
        // Only emit flags that are true or that the model must act on — every
        // token here is paid for.
        if r.kind == AssetKind::Model && r.rigged {
            s.push_str(",\"rigged\":true");
        }
        if r.loops {
            s.push_str(",\"loops\":true");
        }
        if !r.playable {
            s.push_str(",\"playable\":false");
        }
        if let Some(sz) = r.size {
            s.push_str(&format!(
                ",\"size\":[{:.2},{:.2},{:.2}]",
                sz[0], sz[1], sz[2]
            ));
        }
        s.push('}');
    }
    s.push(']');
    s
}

/// A short blurb for the system prompt. Must stay tiny however large the
/// library grows, so it summarises by top-level category with counts and names
/// only a handful of examples — never the catalogue.
pub fn library_summary(index: &AssetIndex) -> String {
    if index.is_empty() {
        return "No stock assets are installed (run apps/arcade/download_assets.sh). \
                Build scenes from primitive shapes and the built-in synth."
            .to_string();
    }
    // Roll categories up to their top level so the list stays a sentence.
    let mut tops: Vec<(String, usize)> = Vec::new();
    for (cat, n) in index.category_counts() {
        let top = cat.split('/').next().unwrap_or(&cat).to_string();
        match tops.iter_mut().find(|(t, _)| *t == top) {
            Some((_, c)) => *c += n,
            None => tops.push((top, n)),
        }
    }
    tops.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let cats: Vec<String> = tops.iter().map(|(t, n)| format!("{t} ({n})")).collect();

    // A few concrete examples make the id shape obvious without listing much.
    let examples: Vec<&str> = index
        .entries()
        .iter()
        .filter(|e| !e.categories.is_empty())
        .step_by((index.len() / 3).max(1))
        .take(3)
        .map(|e| e.id.as_str())
        .collect();

    let models = index.count_of(AssetKind::Model);
    let sounds = index.count_of(AssetKind::Sound);
    let music = index.count_of(AssetKind::Music);
    format!(
        "A stock CC0 asset library is available: {models} models, {sounds} sounds, {music} music \
         jingles — {}. Call find_model(query) to search it in plain language (models, sounds and \
         music together; filter with kind) — never guess an asset id. Ids look like {}.",
        cats.join(", "),
        examples.join(", ")
    )
}

/// What the local librarian tier produces: a resolved model plus a structured
/// spawn action, so a small model can answer "put a truck in the game" without
/// writing any script.
#[derive(Debug, PartialEq)]
pub struct SpawnAction {
    pub model_id: String,
    pub name: String,
    /// Confidence in 0.0..=1.0 that this is what was asked for. The app
    /// decides the threshold: act locally when high, escalate when low.
    pub confidence: f32,
}

/// Resolve a plain-language request to a spawn action, or `None` when the
/// library has nothing plausible — in which case the caller should escalate to
/// the cloud agent rather than spawn something wrong.
pub fn local_spawn(index: &AssetIndex, request: &str) -> Option<SpawnAction> {
    let (entry, confidence) = index.best(request)?;
    Some(SpawnAction {
        model_id: entry.id.clone(),
        name: entry.name.clone(),
        confidence,
    })
}

/// Attribution lines for every source present in the library, for a published
/// game package to carry.
pub fn credits(index: &AssetIndex) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for src in [Source::Kenney, Source::KayKit] {
        if index.entries().iter().any(|e| e.source == src) && !out.contains(&src.credit()) {
            out.push(src.credit());
        }
    }
    out
}

// ---------------------------------------------------------------- kit lookup

/// A kit is a coherent, visually-matching SET of tiles. A level assembled from
/// one tile of each of five kits looks like a junk drawer; a level built from
/// one kit looks designed. So the AI needs to discover that a set exists —
/// and its grid pitch — before it can lay anything out.
pub const FIND_KIT: ToolDescriptor = ToolDescriptor {
    name: "find_kit",
    description: "List modular building kits — sets of tiles designed to snap together on a \
                  grid (roads, dungeons, caves, buildings, race tracks, platformer blocks). \
                  Returns each kit's id, its tile size (the grid pitch to place tiles on) and \
                  which roles it provides (straight, corner, junction, end, ramp, wall, floor, \
                  door, ...). Build a level from ONE kit so the pieces match visually, then \
                  call find_model with that kit's name to get specific tile ids.",
    params: &[
        ToolParam {
            name: "query",
            ty: "string",
            description: "Optional: what kind of level, e.g. \"road\", \"dungeon\", \"city\", \
                          \"race track\", \"cave\", \"platformer\".",
            required: false,
        },
        ToolParam {
            name: "role",
            ty: "string",
            description: "Optional: only kits providing this piece, e.g. \"junction\".",
            required: false,
        },
    ],
};

/// Execute `find_kit`. Compact by design: kit id, tile size, and role counts —
/// enough to plan a layout, without spending tokens on individual tile ids
/// (those come from a follow-up `find_model` scoped to the kit).
pub fn execute_kit(index: &AssetIndex, query: Option<&str>, role: Option<&str>) -> Vec<KitResult> {
    let q = query.unwrap_or("").to_ascii_lowercase();
    let mut out: Vec<KitResult> = index
        .kits()
        .into_iter()
        .filter(|k| {
            role.is_none_or(|r| k.roles.iter().any(|(kr, _)| kr == r || kr.starts_with(r)))
        })
        .filter(|k| {
            q.is_empty()
                || q.split_whitespace().any(|w| {
                    k.pack.contains(w) || k.name.to_ascii_lowercase().contains(w)
                })
        })
        .map(|k| KitResult {
            id: k.pack,
            name: k.name,
            tiles: k.tiles,
            tile_size: k.tile_size,
            roles: k.roles,
        })
        .collect();
    // Most roles first: a kit with junctions and ramps composes into more
    // interesting levels than one with a single straight piece.
    out.sort_by(|a, b| b.roles.len().cmp(&a.roles.len()).then(a.id.cmp(&b.id)));
    out
}

pub struct KitResult {
    pub id: String,
    pub name: String,
    pub tiles: u32,
    pub tile_size: Option<f32>,
    pub roles: Vec<(String, u32)>,
}

pub fn kits_to_json(kits: &[KitResult]) -> String {
    let mut s = String::from("[");
    for (i, k) in kits.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("{{\"id\":\"{}\",\"tiles\":{}", k.id, k.tiles));
        if let Some(t) = k.tile_size {
            s.push_str(&format!(",\"tile_size\":{t:.2}"));
        }
        s.push_str(",\"roles\":{");
        for (j, (r, n)) in k.roles.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            s.push_str(&format!("\"{r}\":{n}"));
        }
        s.push_str("}}");
    }
    s.push(']');
    s
}
