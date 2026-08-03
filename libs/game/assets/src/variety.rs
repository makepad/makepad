//! Variety and palette selection — turning a search engine into a composition
//! tool.
//!
//! The ranking was never the problem. Asking for "suburban house building"
//! already returns 21 distinct houses at equal score, and "pine tree" returns
//! six distinct pines. The problem was that [`crate::AssetIndex::find`] hands
//! back a ranked list, the caller takes hit #1, and places it N times — so a
//! village came out as five identical houses on five identical lots. With 4400
//! models installed, a scene used about six of them.
//!
//! So the fix is an API that *affords* variety rather than better scoring:
//! ask for N models and get N DIFFERENT ones, or ask for a palette and get a
//! set that visually belongs together.

use crate::{AssetEntry, AssetIndex, Filters, Hit};

/// How different the returned models should be from each other.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Spread {
    /// Round-robin across variant families, then within them. Maximum spread
    /// of shapes first, filled out with variants of those shapes. Never
    /// repeats a model.
    ///
    /// The right default, and it handles both real cases with one rule: a
    /// village asking for 5 houses gets `building-type-a..e` (one family, five
    /// members), while a forest asking for 5 trees gets pine, oak, palm and
    /// two more species before it takes a second pine.
    #[default]
    Mixed,
    /// At most one model per family — maximum shape diversity, fewer results.
    /// For when repetition of a silhouette is the thing to avoid.
    Kinds,
    /// All from the best-matching family: `building-type-a`, `-b`, `-c`. A
    /// coherent row of the same kind of thing.
    Variants,
}

/// Parameters for a variety query.
#[derive(Clone, Debug)]
pub struct VarietyParams {
    pub count: usize,
    pub spread: Spread,
    /// Seeds the choice WITHIN families, so a re-run of the same game places
    /// the same models and two different seeds place different ones. Never the
    /// world rng: selection has to be reproducible from (query, seed) alone
    /// for multiplayer to replicate a scene without shipping model lists.
    pub seed: u64,
    pub filters: Filters,
}

impl Default for VarietyParams {
    fn default() -> Self {
        VarietyParams { count: 5, spread: Spread::Mixed, seed: 0, filters: Filters::default() }
    }
}

impl VarietyParams {
    pub fn new(count: usize) -> Self {
        VarietyParams { count, ..Default::default() }
    }
    pub fn spread(mut self, s: Spread) -> Self {
        self.spread = s;
        self
    }
    pub fn seed(mut self, s: u64) -> Self {
        self.seed = s;
        self
    }
}

/// A coherent set of models drawn from ONE pack, grouped by what each is for.
///
/// This is what a composer actually wants: not "the best house" but "houses,
/// trees, fences and street furniture that look like they come from the same
/// game". Drawing from one pack is what guarantees that — Kenney authors each
/// pack as a matched set.
#[derive(Clone, Debug)]
pub struct Palette {
    pub pack: String,
    pub name: String,
    /// Group name (the family, e.g. "building", "fence") → model ids, most
    /// relevant first.
    pub groups: Vec<(String, Vec<String>)>,
}

impl Palette {
    /// Ids in a named group, empty when the pack has nothing of that sort.
    pub fn group(&self, name: &str) -> &[String] {
        self.groups
            .iter()
            .find(|(g, _)| g == name)
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[])
    }
    pub fn total(&self) -> usize {
        self.groups.iter().map(|(_, v)| v.len()).sum()
    }
}

/// The variant family a model belongs to: its name with variant markers
/// stripped.
///
/// Kenney names variants systematically, but the SAME suffix means different
/// things in different packs — `building-type-a`..`-u` are twenty-one
/// genuinely different house designs, while `tree_blocks` / `tree_blocks_dark`
/// is one tree in two colours. A name cannot tell those apart, and guessing
/// wrong in either direction hurts: collapse too much and a village has one
/// house, collapse too little and a "forest" is five recolours of one trunk.
///
/// So the family is deliberately coarse — it drops only markers that are
/// certainly not descriptive (single letters, digits, "default"/"type") via
/// the same noise rule the indexer uses — and the SELECTION strategy does the
/// rest. `Mixed` draws across families before drawing within one, so it is
/// correct whichever way a pack happens to be named.
pub fn family_of(entry: &AssetEntry) -> String {
    let stem = entry.id.rsplit('/').next().unwrap_or(&entry.id);
    let tokens: Vec<String> = crate::split_ident(stem)
        .into_iter()
        .filter(|t| !crate::packs::is_noise_token(t))
        .filter(|t| !is_reskin_token(t))
        .collect();
    if tokens.is_empty() {
        stem.to_string()
    } else {
        tokens.join(" ")
    }
}

/// Tokens that mark a RE-SKIN rather than a different shape: colours, seasons
/// and lighting words. `tree_blocks`, `tree_blocks_dark` and `tree_blocks_fall`
/// are one tree in three palettes, and treating them as three "kinds" made a
/// request for six kinds of tree return the same silhouette six times.
///
/// Colour is deliberately included even though "a red car" is a real request —
/// dropping it only affects GROUPING, and `Mixed` still returns the red and
/// blue cars as distinct members of one family. What it prevents is a
/// "variety" of six identical cars in six colours being mistaken for variety.
fn is_reskin_token(t: &str) -> bool {
    matches!(
        t,
        "dark" | "light" | "fall" | "autumn" | "winter" | "summer" | "spring" | "snow"
            | "red" | "blue" | "green" | "yellow" | "orange" | "purple" | "pink" | "white"
            | "black" | "grey" | "gray" | "brown" | "teal" | "beige" | "tan"
    )
}

/// A coarse bucket for palette grouping: the first meaningful token, or the
/// tile role when the pack is a modular kit.
///
/// [`family_of`] is the wrong key here — it is deliberately specific, and on a
/// 300-model pack it produced 167 groups of one id each, which is a listing
/// rather than a palette. A composer wants "buildings", "trees", "fences",
/// not "balcony wall fence".
fn group_of(entry: &AssetEntry) -> String {
    if let Some(role) = entry.role {
        return role.to_string();
    }
    crate::split_ident(entry.id.rsplit('/').next().unwrap_or(&entry.id))
        .into_iter()
        .find(|t| !crate::packs::is_noise_token(t) && !is_reskin_token(t))
        .unwrap_or_else(|| "misc".to_string())
}

/// Deterministic per-(seed, id) key. FNV-1a so the same seed always yields the
/// same order and a different seed yields a genuinely different one, without
/// carrying any RNG state around.
fn shuffle_key(seed: u64, id: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed.wrapping_mul(0x100_0000_01b3);
    for b in id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// The pack the query most belongs to, by summed relevance of its hits. Summed
/// rather than counted so two strong matches beat ten weak brushes.
fn dominant_pack(hits: &[Hit<'_>]) -> Option<String> {
    let mut packs: Vec<(&str, u32)> = Vec::new();
    for h in hits.iter().take(60) {
        let p = h.entry.pack.as_str();
        match packs.iter_mut().find(|(name, _)| *name == p) {
            Some((_, s)) => *s += h.score,
            None => packs.push((p, h.score)),
        }
    }
    packs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    packs.first().map(|(p, _)| p.to_string())
}

/// Group ranked hits into families, preserving each family's best score and
/// the ranked order of its members.
///
/// Families from the query's dominant pack come first. Without that, asking
/// for five houses returned one house from each of five packs — a suburban
/// house, a hex-tile house, a modular house and a driveway — which is the
/// junk-drawer failure that variety was supposed to fix, arrived at from the
/// opposite direction. Crossing packs is still allowed once the best pack runs
/// out, because a wider net beats returning nothing.
fn group_families<'a>(
    hits: &[Hit<'a>],
    seed: u64,
    prefer_pack: Option<&str>,
) -> Vec<(String, u32, Vec<&'a AssetEntry>)> {
    let mut groups: Vec<(String, u32, bool, Vec<&'a AssetEntry>)> = Vec::new();
    for h in hits {
        let fam = family_of(h.entry);
        let native = prefer_pack.is_some_and(|p| p == h.entry.pack);
        match groups.iter_mut().find(|(f, _, _, _)| *f == fam) {
            Some((_, best, is_native, members)) => {
                *best = (*best).max(h.score);
                *is_native |= native;
                members.push(h.entry);
            }
            None => groups.push((fam, h.score, native, vec![h.entry])),
        }
    }
    // Dominant pack first, then relevance; members shuffled by seed so a
    // different seed picks different members of an equally-good family.
    groups.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut out: Vec<(String, u32, Vec<&'a AssetEntry>)> = Vec::new();
    for (fam, score, _, mut members) in groups {
        members.sort_by_key(|e| (shuffle_key(seed, &e.id), e.id.clone()));
        out.push((fam, score, members));
    }
    out
}

impl AssetIndex {
    /// Find N *distinct* models for one query.
    ///
    /// This is the call a composer wants and [`AssetIndex::find`] is not: it
    /// never returns the same model twice, and by default it spreads across
    /// variant families before repeating a shape.
    pub fn find_many(&self, query: &str, params: &VarietyParams) -> Vec<&AssetEntry> {
        if params.count == 0 {
            return Vec::new();
        }
        // Pull a generous candidate pool: enough to have several families to
        // spread across, and cheap because the inverted index already narrowed
        // it. 12x is empirical — a village asking for 5 houses wants to see
        // all 21 building-types, not the first 5.
        let pool = (params.count * 12).clamp(24, 400);
        let hits: Vec<Hit> = self
            .find_filtered(query, &params.filters)
            .into_iter()
            .take(pool)
            .collect();
        if hits.is_empty() {
            return Vec::new();
        }
        let prefer = dominant_pack(&hits);
        let mut groups = group_families(&hits, params.seed, prefer.as_deref());

        // Variety must not drift off-topic. Round-robin is right when the
        // families are kinds of the SAME thing (pine, oak, palm) and wrong
        // when they are different things sharing a pack theme: asking for five
        // houses returned one house then two driveways and two fences, because
        // `city-kit-suburban` themes all of them "house".
        //
        // Scoring cannot separate those — an exact one-word hit (`tree`)
        // outscores a compound sibling (`tree_blocks`) purely for being
        // shorter, so a relevance band cuts real variety while keeping the
        // drift. What actually distinguishes them is whether the family IS the
        // thing asked for: "tree", "tree blocks" and "tree cone" all name a
        // tree; "driveway" and "fence" do not name a house.
        //
        // Applied only when it leaves something, because a functional query
        // ("somewhere to hide") names no noun the filenames share, and there
        // the ranking's own judgement is all we have.
        let named: Vec<(String, u32, Vec<&AssetEntry>)> = {
            let terms = crate::tokenize(query);
            groups
                .iter()
                .filter(|(fam, _, _)| {
                    terms.iter().any(|t| {
                        fam.split_whitespace()
                            .any(|w| w == t || crate::stem(t).is_some_and(|s| w == s))
                    })
                })
                .cloned()
                .collect()
        };
        if !named.is_empty() {
            groups = named;
        }

        let mut out: Vec<&AssetEntry> = Vec::new();
        match params.spread {
            Spread::Kinds => {
                for (_, _, members) in &groups {
                    if out.len() >= params.count {
                        break;
                    }
                    if let Some(first) = members.first() {
                        out.push(first);
                    }
                }
            }
            Spread::Variants => {
                if let Some((_, _, members)) = groups.first() {
                    out.extend(members.iter().take(params.count));
                }
            }
            Spread::Mixed => {
                // Round-robin: one from each family, then a second from each,
                // and so on — the widest spread the pool allows before any
                // shape repeats.
                //
                // Run it over the dominant pack FIRST and only spill into
                // other packs if that pack cannot fill the count. Breadth
                // across packs is not free: five houses drawn from five packs
                // are five art styles in one street. `city-kit-suburban` alone
                // holds 21 house designs, so a village never needs to leave it.
                let native: Vec<_> = groups
                    .iter()
                    .filter(|(_, _, m)| m.first().is_some_and(|e| Some(&e.pack) == prefer.as_ref()))
                    .collect();
                let foreign: Vec<_> = groups
                    .iter()
                    .filter(|(_, _, m)| m.first().is_none_or(|e| Some(&e.pack) != prefer.as_ref()))
                    .collect();
                for stage in [native, foreign] {
                    if out.len() >= params.count {
                        break;
                    }
                    let deepest = stage.iter().map(|(_, _, m)| m.len()).max().unwrap_or(0);
                    'stage: for depth in 0..deepest {
                        for (_, _, members) in &stage {
                            if let Some(e) = members.get(depth) {
                                out.push(e);
                                if out.len() >= params.count {
                                    break 'stage;
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// A coherent set of models drawn from ONE pack, for building a scene that
    /// looks authored rather than assembled from a junk drawer.
    ///
    /// The pack is chosen by which one the query's best hits actually come
    /// from, so "a village" lands on a village pack and takes its houses,
    /// fences and props together.
    pub fn palette(&self, query: &str, seed: u64) -> Option<Palette> {
        // Which pack owns this query? Score by summed relevance of its hits,
        // not by count, so a pack with two perfect matches beats one with ten
        // weak brushes.
        let hits = self.find(query);
        if hits.is_empty() {
            return None;
        }
        let mut packs: Vec<(&str, u32)> = Vec::new();
        for h in hits.iter().take(60) {
            let p = h.entry.pack.as_str();
            match packs.iter_mut().find(|(name, _)| *name == p) {
                Some((_, s)) => *s += h.score,
                None => packs.push((p, h.score)),
            }
        }
        packs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let pack = packs.first()?.0.to_string();

        // Everything that pack ships, bucketed by family — that IS the palette.
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        let mut members: Vec<&AssetEntry> = self
            .entries()
            .iter()
            .filter(|e| e.pack == pack && e.kind == crate::AssetKind::Model)
            .collect();
        members.sort_by_key(|e| (shuffle_key(seed, &e.id), e.id.clone()));
        for e in members {
            let g = group_of(e);
            match groups.iter_mut().find(|(name, _)| *name == g) {
                Some((_, ids)) => ids.push(e.id.clone()),
                None => groups.push((g, vec![e.id.clone()])),
            }
        }
        // Biggest groups first: they are what the pack is mostly about.
        groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        let name = crate::packs::theme_of(&pack)
            .map(|t| t.name.to_string())
            .unwrap_or_else(|| pack.replace('-', " "));
        Some(Palette { pack, name, groups })
    }
}
