//! Query-side synonym expansion: the word a person types, widened to the
//! words an annotation actually used.
//!
//! Two tables, one rule:
//! - [`wordnet`] is the vendored Princeton WordNet 3.1 extract — broad,
//!   generic, generated, never hand-edited.
//! - `CURATED` below is a small hand-written game/asset-domain overlay. It
//!   WINS: when a word appears in it, WordNet is not consulted for that word
//!   at all. WordNet knows that a puppy is a young dog, not that a catalog
//!   searcher typing `puppy` wants the dog mesh; it also knows fourteen other
//!   senses of `dog` that nobody browsing 3D props means.
//!
//! Everything here is pure, allocation-light and deterministic: the same term
//! always yields the same expansion list in the same order, because both
//! tables are fixed and sorted at build time. Nothing reads the clock, the
//! catalog or the environment. The index is never involved — expansion is a
//! query-time widening only, so no annotation is ever reindexed for it.
//!
//! Expansion order for a term (the caller truncates, never reorders):
//!   1. plural/singular folds of the term itself,
//!   2. synonyms of the term,
//!   3. synonyms of each fold, in fold order,
//!   4. plural/singular folds of everything from 2 and 3.
//! Synonyms come before their own plurals so a cap can only ever cost the
//! weakest widening, never the word itself.

mod wordnet;

use crate::search::MAX_TERM_BYTES;

/// Shortest expansion word worth producing. One-letter folds ("as" -> "a")
/// are noise, not language.
const MIN_TERM_BYTES: usize = 2;

/// Hand-written domain overlay, checked before WordNet and overriding it.
///
/// These are not dictionary synonyms; they are "same thing, as an asset
/// catalog means it" — a searcher typing the left word wants the right rows.
/// Groups may overlap (a query word collects every group it appears in, in
/// table order); every member must be a lowercase ASCII-alphanumeric token
/// of 2..=32 bytes, i.e. something `search::tokenize_into` can produce.
/// `curated_table_is_well_formed` proves both.
static CURATED: &[&[&str]] = &[
    // -- creatures ---------------------------------------------------------
    &["dog", "puppy", "pup", "doggy", "doggie", "hound", "canine"],
    &["cat", "kitten", "kitty", "feline"],
    &["ghost", "spirit", "phantom", "specter", "spectre", "wraith"],
    &["zombie", "undead", "ghoul"],
    &["skeleton", "skull", "bones", "skeletal"],
    &["monster", "creature", "beast", "fiend"],
    &["dragon", "wyvern", "drake"],
    &["character", "avatar", "figure", "npc"],
    &["soldier", "warrior", "fighter", "knight", "guard", "trooper"],
    &["wizard", "mage", "sorcerer", "witch", "magician", "warlock"],
    &["alien", "extraterrestrial", "ufo", "martian"],
    &["robot", "droid", "android", "mech", "bot"],
    // -- weapons -----------------------------------------------------------
    &[
        "weapon", "gun", "firearm", "rifle", "pistol", "handgun", "blaster", "shotgun",
        "revolver", "sniper",
    ],
    &["sword", "blade", "katana", "saber", "sabre", "machete"],
    &["axe", "ax", "hatchet"],
    &["bow", "crossbow", "archery"],
    &["shield", "buckler", "armor", "armour"],
    &["helmet", "helm", "hat", "cap"],
    // -- vehicles ----------------------------------------------------------
    &["car", "auto", "automobile", "sedan", "coupe"],
    &["race", "racing", "racer", "sports", "sport"],
    &["truck", "lorry", "van", "pickup"],
    &["motorcycle", "motorbike", "bike", "bicycle"],
    &["boat", "ship", "vessel", "raft", "canoe"],
    &["airplane", "plane", "aircraft", "jet", "biplane"],
    // -- furniture and props ----------------------------------------------
    &["chair", "seat", "stool", "bench"],
    &["sofa", "couch", "settee", "loveseat"],
    &["table", "desk", "workbench"],
    &["cabinet", "cupboard", "closet", "wardrobe", "dresser"],
    &["lamp", "lantern", "light", "torch", "candle"],
    &["barrel", "drum", "cask", "keg"],
    &["crate", "box", "carton", "chest", "container"],
    &["bag", "sack", "pouch", "backpack"],
    &["coin", "gold", "treasure", "money", "loot"],
    &["gem", "gemstone", "jewel", "crystal", "diamond"],
    &["potion", "flask", "vial", "elixir", "bottle"],
    &["food", "meal", "snack"],
    // -- buildings and terrain --------------------------------------------
    &["house", "home", "cottage", "cabin", "hut", "shack"],
    &["castle", "fortress", "fort", "citadel", "keep"],
    &["tower", "spire", "turret"],
    &["door", "doorway", "gate", "gateway", "hatch"],
    &["window", "pane", "casement"],
    &["fence", "railing", "palisade"],
    &["sign", "signpost", "billboard", "placard", "banner"],
    &["road", "street", "path", "pathway", "trail"],
    &["bridge", "overpass", "viaduct"],
    &["stairs", "staircase", "steps", "stairway"],
    &["gravestone", "headstone", "tombstone", "grave", "tomb"],
    &["rock", "stone", "boulder", "pebble"],
    &["mountain", "hill", "cliff", "peak"],
    &["tree", "pine", "oak", "spruce", "birch", "willow"],
    &["bush", "shrub", "hedge"],
    &["leaves", "leaf", "foliage", "leafage"],
    &["plant", "vegetation", "greenery", "flora"],
    &["flower", "blossom", "bloom"],
    &["grass", "turf", "lawn", "meadow"],
    &["water", "liquid", "fluid", "ocean", "sea"],
    &["fire", "flame", "campfire", "bonfire", "blaze"],
    &["sand", "desert", "dune"],
    &["snow", "ice", "frozen", "icy"],
    // -- size, condition, look --------------------------------------------
    &["small", "tiny", "little", "mini", "miniature", "petite", "compact"],
    &["big", "large", "huge", "giant", "gigantic", "massive", "oversized"],
    &["creepy", "spooky", "eerie", "scary", "haunted", "ghostly", "sinister", "halloween"],
    &["old", "ancient", "antique", "aged", "worn", "weathered"],
    &["new", "modern", "contemporary"],
    &["broken", "damaged", "cracked", "destroyed", "ruined", "wrecked"],
    &["rusty", "rusted", "corroded"],
    &["dark", "black", "shadowy", "gloomy", "murky"],
    &["bright", "shiny", "glossy", "glowing", "luminous"],
    &["round", "circular", "spherical", "curved"],
    &["square", "boxy", "cubic", "rectangular", "cube"],
    &["metal", "steel", "iron", "metallic"],
    &["wood", "wooden", "timber", "lumber"],
    &["brick", "masonry"],
    // -- style -------------------------------------------------------------
    &["cartoon", "cartoony", "stylized", "stylised", "toon"],
    &["realistic", "photoreal", "photorealistic", "detailed"],
    &["lowpoly", "voxel", "blocky", "faceted"],
    &["pixel", "pixelated", "pixelart", "retro"],
    &["scifi", "futuristic", "space", "cyberpunk", "cyber"],
    &["fantasy", "medieval", "magical", "mythical"],
    &["military", "army", "combat", "war", "tactical"],
];

/// Trivial English number folds, both directions: `dogs` <-> `dog`,
/// `boxes` <-> `box`, `bunnies` <-> `bunny`. Not a stemmer — nothing here
/// changes a word's stem, so a fold is always a word a writer could have
/// typed, and the caller scores folds in the same lowered tier as synonyms.
fn plural_folds(term: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Singular -> plural. A term that already ends in `s` gets no plural of
    // its own plural ("dogses" is nobody's word).
    if !term.ends_with('s') {
        if term.ends_with('y') && term.len() >= 3 {
            out.push(format!("{}ies", &term[..term.len() - 1]));
        } else if ["x", "z", "ch", "sh"].iter().any(|s| term.ends_with(s)) {
            out.push(format!("{term}es"));
        } else {
            out.push(format!("{term}s"));
        }
    }
    // Plural -> singular.
    if let Some(stem) = term.strip_suffix("ies") {
        if stem.len() >= 2 {
            out.push(format!("{stem}y"));
        }
    }
    if let Some(stem) = term.strip_suffix("es") {
        if stem.len() >= MIN_TERM_BYTES
            && ["s", "x", "z", "ch", "sh"].iter().any(|s| stem.ends_with(s))
        {
            out.push(stem.to_string());
        }
    }
    if let Some(stem) = term.strip_suffix('s') {
        if stem.len() >= MIN_TERM_BYTES && !stem.ends_with('s') {
            out.push(stem.to_string());
        }
    }
    out.retain(|w| (MIN_TERM_BYTES..=MAX_TERM_BYTES).contains(&w.len()) && w != term);
    out.dedup();
    out
}

/// Every curated group containing `word`, in table order.
fn curated_of(word: &str, out: &mut Vec<String>) -> bool {
    let mut hit = false;
    for group in CURATED {
        if group.contains(&word) {
            hit = true;
            for w in *group {
                if *w != word {
                    out.push((*w).to_string());
                }
            }
        }
    }
    hit
}

/// The word starting at `off` in the WordNet blob: up to the next separator.
fn word_at(off: u32) -> &'static str {
    let blob = wordnet::WORDNET_BLOB.as_bytes();
    let start = off as usize;
    let mut end = start;
    while end < blob.len() && blob[end] != b' ' && blob[end] != b'\n' {
        end += 1;
    }
    // The blob is pure ASCII (`table_is_ascii_and_indexed` proves it), so
    // every byte offset is a char boundary.
    &wordnet::WORDNET_BLOB[start..end]
}

/// The whole group line containing the word slot at `off`.
fn group_at(off: u32) -> &'static str {
    let blob = wordnet::WORDNET_BLOB;
    let off = off as usize;
    let start = blob[..off].rfind('\n').map_or(0, |i| i + 1);
    let end = blob[off..].find('\n').map_or(blob.len(), |i| off + i);
    &blob[start..end]
}

/// Members of every WordNet group containing `word`, in blob order.
fn wordnet_of(word: &str, out: &mut Vec<String>) {
    let index = wordnet::WORDNET_INDEX;
    let mut i = index.partition_point(|&off| word_at(off) < word);
    while i < index.len() && word_at(index[i]) == word {
        for w in group_at(index[i]).split(' ') {
            if w != word {
                out.push(w.to_string());
            }
        }
        i += 1;
    }
}

/// Ordered expansion candidates for one query term, never including the term
/// itself and never repeating a word. The caller applies its own caps and
/// drops anything already claimed by another term's group.
pub(crate) fn expand_term(term: &str) -> Vec<String> {
    let folds = plural_folds(term);
    // The term's own synonyms, curated first and exclusively when present.
    let mut syn: Vec<String> = Vec::new();
    if !curated_of(term, &mut syn) {
        wordnet_of(term, &mut syn);
    }
    // Then the folds' synonyms: `dogs` should reach `puppy` too.
    for f in &folds {
        if !curated_of(f, &mut syn) {
            wordnet_of(f, &mut syn);
        }
    }
    let mut out: Vec<String> = folds.clone();
    out.extend(syn.iter().cloned());
    // Finally the synonyms' own plurals: an annotation is as likely to say
    // "three dogs" as "dog", and `puppy` should reach both.
    for w in &syn {
        out.extend(plural_folds(w));
    }
    out.retain(|w| (MIN_TERM_BYTES..=MAX_TERM_BYTES).contains(&w.len()) && w != term);
    let mut seen: Vec<String> = Vec::with_capacity(out.len());
    out.retain(|w| {
        if seen.iter().any(|s| s == w) {
            return false;
        }
        seen.push(w.clone());
        true
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated table's two halves must agree: ASCII everywhere, every
    /// index offset at a word start, and the index sorted by that word (the
    /// binary search in `wordnet_of` is only correct if it is).
    #[test]
    fn table_is_ascii_and_indexed() {
        let blob = wordnet::WORDNET_BLOB;
        assert!(blob.is_ascii(), "blob must be ASCII");
        assert!(!blob.is_empty());
        for b in blob.bytes() {
            assert!(
                b.is_ascii_lowercase() || b.is_ascii_digit() || b == b' ' || b == b'\n',
                "unexpected byte {b:?} in blob"
            );
        }
        let index = wordnet::WORDNET_INDEX;
        assert_eq!(index.len(), blob.split('\n').filter(|l| !l.is_empty()).map(|l| l.split(' ').count()).sum::<usize>());
        let mut prev = "";
        for &off in index {
            let off = off as usize;
            assert!(off == 0 || blob.as_bytes()[off - 1] == b' ' || blob.as_bytes()[off - 1] == b'\n');
            let w = word_at(off as u32);
            assert!(!w.is_empty());
            assert!(prev <= w, "index not sorted: {prev} > {w}");
            assert!(group_at(off as u32).split(' ').any(|m| m == w));
            prev = w;
        }
    }

    /// The overlay is hand-written, so its shape is asserted, not assumed.
    #[test]
    fn curated_table_is_well_formed() {
        for group in CURATED {
            assert!(group.len() >= 2, "singleton curated group: {group:?}");
            for (i, w) in group.iter().enumerate() {
                assert!(
                    (MIN_TERM_BYTES..=MAX_TERM_BYTES).contains(&w.len())
                        && w.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
                    "curated word {w:?} is not a token the tokenizer can produce"
                );
                assert!(!group[..i].contains(w), "duplicate {w:?} in {group:?}");
            }
        }
    }

    #[test]
    fn curated_wins_over_wordnet() {
        // `dog` is dropped from WordNet as polysemy noise; the overlay is why
        // `puppy` and `dog` find each other at all.
        assert!(expand_term("puppy").iter().any(|w| w == "dog"));
        assert!(expand_term("dog").iter().any(|w| w == "puppy"));
        // `light` is a curated lamp word AND all over WordNet; curated wins,
        // so the expansion stays about lamps.
        let light = expand_term("light");
        assert!(light.iter().any(|w| w == "lamp"));
        assert!(!light.iter().any(|w| w == "lightness"));
    }

    #[test]
    fn wordnet_carries_the_long_tail() {
        assert!(expand_term("auto").iter().any(|w| w == "car"));
        assert!(expand_term("cask").iter().any(|w| w == "barrel"));
        assert!(expand_term("stone").iter().any(|w| w == "rock"));
    }

    #[test]
    fn plural_folds_go_both_ways() {
        assert!(expand_term("dogs").iter().any(|w| w == "dog"));
        assert!(expand_term("dog").iter().any(|w| w == "dogs"));
        assert!(expand_term("boxes").iter().any(|w| w == "box"));
        assert!(expand_term("bunnies").iter().any(|w| w == "bunny"));
        // A fold reaches the fold's synonyms too, and a synonym its plural.
        assert!(expand_term("puppies").iter().any(|w| w == "dog"));
        assert!(expand_term("puppy").iter().any(|w| w == "dogs"));
        // Synonyms are never pushed below their own plurals by the cap.
        let e = expand_term("puppy");
        assert!(
            e.iter().position(|w| w == "dog") < e.iter().position(|w| w == "dogs"),
            "{e:?}"
        );
        // No stemming: nothing invents a word shorter than two bytes.
        assert!(expand_term("as").iter().all(|w| w.len() >= MIN_TERM_BYTES));
    }

    #[test]
    fn expansion_is_deterministic_and_unique() {
        for term in ["dog", "car", "tiny", "leaves", "unknownwordxyz"] {
            let a = expand_term(term);
            assert_eq!(a, expand_term(term), "{term} expanded differently twice");
            let mut sorted = a.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), a.len(), "{term} expanded with duplicates");
            assert!(!a.iter().any(|w| w == term));
        }
    }
}
