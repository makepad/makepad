//! Query-side synonym expansion: the word a person types finding the word the
//! annotation used, without ever touching the index.
//!
//! The fixture copies the shape of the live catalog's annotations — short
//! construction lines like "dog pet; standalone; 1x1; brown/grey; cube-shaped
//! dog with floppy ears" — because that is the text expansion has to reach.

mod common;
use common::*;
use makepad_asset_store::{
    AssetAnnotation, AssetServerCore, SearchFilters, SearchQuery, SearchViewer, ServerError,
    ViewerScope, Visibility,
};
use makepad_asset_data::AssetId;

const ANYONE: SearchViewer<'static> = SearchViewer { principal: None, scope: ViewerScope::All };

fn ann(title: &str, description: &str) -> AssetAnnotation {
    AssetAnnotation {
        title: title.into(),
        description: description.into(),
        kind: None,
        categories: Vec::new(),
        tags: Vec::new(),
        creator: String::new(),
        artist: String::new(),
        artist_url: String::new(),
        album: String::new(),
        source_url: String::new(),
        license: String::new(),
        license_url: String::new(),
        owner: None,
        generator: String::new(),
        backend: String::new(),
        model: String::new(),
        prompt: String::new(),
        provenance: String::new(),
        visibility: Visibility::Public,
    }
}

/// Expanding query: what the HTTP routes send unless asked for `exact=1`.
fn q(text: &str) -> SearchQuery<'_> {
    SearchQuery {
        text,
        filters: SearchFilters::default(),
        expand: true,
        page_size: 10,
        newest: false,
        facets: 0,
    }
}

/// The escape hatch: the typed words alone.
fn q_exact(text: &str) -> SearchQuery<'_> {
    SearchQuery { expand: false, ..q(text) }
}

fn add(core: &AssetServerCore, n: u8, title: &str, description: &str) -> AssetId {
    let id = asset_id_n(n);
    core.catalog().register_asset(&id, "rik2", NOW).unwrap();
    core.search().set_annotation(&id, &ann(title, description), NOW).unwrap();
    id
}

/// Annotations shaped like the ones the vision pass writes today.
fn fixture(name: &str) -> (std::path::PathBuf, AssetServerCore) {
    let (root, core) = open_core(name);
    add(&core, 1, "Dog", "dog pet; standalone; 1x1; brown/grey; cube-shaped dog with floppy ears");
    add(&core, 2, "Small Dog", "small dog pet; standalone; 1x1; white; low-poly");
    add(&core, 3, "Race Car", "race car vehicle; standalone; 2x1; red; sports styling");
    add(&core, 4, "Blaster Rifle", "blaster rifle weapon; standalone; handheld; grey");
    add(&core, 5, "Gravestone", "gravestone prop; standalone; 1x1; grey; weathered slab");
    add(&core, 6, "Pack", "dogs pack; three dogs on a base; standalone; 2x2; brown");
    add(&core, 7, "Sofa", "couch furniture; standalone; 2x1; green; three seats");
    add(&core, 8, "Leaf Pile", "foliage pile; standalone; 1x1; autumn colours");
    (root, core)
}

/// Titles in rank order.
fn titles(core: &AssetServerCore, query: &SearchQuery<'_>) -> Vec<String> {
    core.search()
        .search(query, &ANYONE, None)
        .unwrap()
        .hits
        .into_iter()
        .map(|h| h.title)
        .collect()
}

/// The same set, order removed: for the "did it find them at all" claims.
fn found(core: &AssetServerCore, query: &SearchQuery<'_>) -> Vec<String> {
    let mut t = titles(core, query);
    t.sort();
    t
}

#[test]
fn a_synonym_finds_the_word_the_annotation_used() {
    let (_root, core) = fixture("syn_finds");
    // The headline case: nothing in the catalog says "puppy".
    assert_eq!(found(&core, &q("puppy")), vec!["Dog", "Pack", "Small Dog"]);
    // Curated overlay: size words, weapon words, gravestone words.
    assert_eq!(titles(&core, &q("tiny")), vec!["Small Dog"]);
    assert_eq!(titles(&core, &q("gun")), vec!["Blaster Rifle"]);
    assert_eq!(titles(&core, &q("headstone")), vec!["Gravestone"]);
    assert_eq!(titles(&core, &q("leaves")), vec!["Leaf Pile"]);
    // WordNet's long tail, no curation involved: auto/car, sofa/couch.
    assert_eq!(titles(&core, &q("automobile")), vec!["Race Car"]);
    assert_eq!(titles(&core, &q("sofa")), vec!["Sofa"]);
    // A word in no table and in no annotation still finds nothing.
    assert!(titles(&core, &q("zeppelin")).is_empty());
}

#[test]
fn plurals_fold_both_ways() {
    let (_root, core) = fixture("syn_plural");
    // Query plural, annotations singular: `dogs` still reaches them.
    assert_eq!(found(&core, &q("dogs")), vec!["Dog", "Pack", "Small Dog"]);
    // Query singular, annotation plural: only "Pack" says "dogs", and the
    // fold finds it — below the assets that literally say "dog", because a
    // fold scores in the expansion tier like any other widening.
    let hits = titles(&core, &q("dog"));
    assert_eq!(hits, vec!["Dog", "Small Dog", "Pack"]);
    // Folding reaches the fold's synonyms too: "puppies" -> "puppy" -> "dog".
    assert!(titles(&core, &q("puppies")).contains(&"Dog".to_string()));
}

#[test]
fn an_exact_hit_always_outranks_a_synonym_only_hit() {
    let (_root, core) = open_core("syn_rank");
    add(&core, 1, "Dog", "dog pet; standalone; 1x1; brown");
    add(&core, 2, "Puppy", "puppy pet; standalone; 1x1; brown");
    let page = core.search().search(&q("dog"), &ANYONE, None).unwrap();
    let got: Vec<(&str, u64)> =
        page.hits.iter().map(|h| (h.title.as_str(), h.score)).collect();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, "Dog");
    assert_eq!(got[1].0, "Puppy");
    assert!(got[0].1 > got[1].1, "exact {got:?} must outscore synonym");
    // The tier is exactly a third, integer-floored: title 100 + description 20
    // against (100 + 20) / 3.
    // The tier divides the POSTING weight, once: this asset's `puppy` posting
    // already sums its fields (title 100 + description 20), and 120 / 3 = 40.
    assert_eq!(got[0].1, 120);
    assert_eq!(got[1].1, 40);
    // And expansion never changes what an exact match scores.
    let exact = core.search().search(&q_exact("dog"), &ANYONE, None).unwrap();
    assert_eq!(exact.hits.len(), 1);
    assert_eq!(exact.hits[0].score, 120);
}

#[test]
fn exact_mode_searches_the_typed_words_alone() {
    let (_root, core) = fixture("syn_exact");
    assert!(titles(&core, &q_exact("puppy")).is_empty());
    assert!(titles(&core, &q_exact("tiny")).is_empty());
    assert!(titles(&core, &q_exact("dogs")).len() < titles(&core, &q("dogs")).len());
    // Exact mode is still a search, not a different one: the literal word
    // returns exactly what it always did.
    assert_eq!(titles(&core, &q_exact("dog")), vec!["Dog", "Small Dog"]);
    // ... and the widened query is the same page with more below it.
    assert_eq!(titles(&core, &q("dog")), vec!["Dog", "Small Dog", "Pack"]);
}

#[test]
fn every_query_term_must_still_be_satisfied_by_its_own_group() {
    let (_root, core) = fixture("syn_conjunction");
    // Both terms match through expansion: tiny -> small, dog -> dog.
    assert_eq!(titles(&core, &q("tiny-dog")), vec!["Small Dog"]);
    // The `-` join grammar of the GET route, and the free text of POST, are
    // the same query.
    assert_eq!(titles(&core, &q("tiny dog")), vec!["Small Dog"]);
    // One thing found is not enough when the query asked for two.
    assert!(titles(&core, &q("puppy-automobile")).is_empty());
    // Two words for ONE thing are one demand, not two: `dog puppy` and
    // `sniper rifle` are names, not conjunctions.
    assert_eq!(titles(&core, &q("dog-puppy")), vec!["Dog", "Small Dog", "Pack"]);
    assert_eq!(titles(&core, &q("sniper-rifle")), vec!["Blaster Rifle"]);
    // The exact word still wins the ranking inside a merged group.
    assert_eq!(titles(&core, &q("puppy-dog"))[0], "Dog");
}

#[test]
fn expansion_is_deterministic_and_never_reindexes() {
    let (_root, core) = fixture("syn_determinism");
    let run = || {
        core.search()
            .search(&q("dog"), &ANYONE, None)
            .unwrap()
            .hits
            .into_iter()
            .map(|h| (h.title, h.score, h.asset_id))
            .collect::<Vec<_>>()
    };
    let first = run();
    assert_eq!(first, run(), "same query, same order, same scores");
    assert_eq!(first, run());
    assert!(first.len() >= 3);
    // Expansion is query-side: searching cannot have moved the index. A
    // cursor embeds the index generation, so identical cursor bytes from
    // before and after a run of searches is that statement, byte for byte.
    let paged = SearchQuery { page_size: 1, ..q("dog") };
    let before = core.search().search(&paged, &ANYONE, None).unwrap().cursor;
    let _ = run();
    let after = core.search().search(&paged, &ANYONE, None).unwrap().cursor;
    assert!(before.is_some());
    assert_eq!(before, after, "a search must not move the index generation");
}

#[test]
fn cursor_pages_are_stable_and_bound_to_the_expansion() {
    let (_root, core) = fixture("syn_cursor");
    let search = core.search();
    let whole = search.search(&q("dog"), &ANYONE, None).unwrap();
    assert!(whole.hits.len() >= 3);

    // Page one hit at a time through the same query.
    let paged = SearchQuery { page_size: 1, ..q("dog") };
    let mut walked = Vec::new();
    let mut cursor: Option<Vec<u8>> = None;
    loop {
        let page = search.search(&paged, &ANYONE, cursor.as_deref()).unwrap();
        assert_eq!(page.total, whole.total);
        walked.extend(page.hits.iter().map(|h| (h.title.clone(), h.score)));
        match page.cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let expected: Vec<(String, u64)> =
        whole.hits.iter().map(|h| (h.title.clone(), h.score)).collect();
    assert_eq!(walked, expected, "keyset paging must replay the total order");

    // The expansion is part of the query shape, so a cursor cut with it on is
    // refused by the same text with `exact=1`.
    let cut = search.search(&paged, &ANYONE, None).unwrap().cursor.expect("more pages");
    let exact_paged = SearchQuery { page_size: 1, ..q_exact("dog") };
    assert!(matches!(
        search.search(&exact_paged, &ANYONE, Some(&cut)).unwrap_err(),
        ServerError::InvalidInput { what: "stale search cursor" }
    ));
}

/// A query at the term budget, fully widened, is still one bounded statement:
/// the caps hold the term list down and the engine runs it.
#[test]
fn a_query_at_the_term_budget_still_runs_expanded() {
    let (_root, core) = fixture("syn_wide");
    let text = "dog cat car tree stone water fire gun sword house chair lamp \
                red blue green small big old new broken dark bright round square \
                metal wood brick sand snow grass leaf boat";
    assert_eq!(text.split_whitespace().count(), 32, "the query-term budget");
    let page = core.search().search(&q(text), &ANYONE, None).unwrap();
    assert_eq!(page.total, 0, "nothing is all of those things at once");
    // One term over the budget still refuses, expansion or not.
    let over = format!("{text} zebra");
    assert!(core.search().search(&q(&over), &ANYONE, None).is_err());

    // A query too wide for the index-seek posting source is served by the
    // flat scanned list instead — slower, same answers.
    let words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima \
                 mike november oscar papa quebec romeo sierra tango uniform victor whiskey \
                 xray yankee";
    assert!(words.split_whitespace().count() > 24, "wider than MAX_SEEK_TERMS");
    add(&core, 40, "Alphabet", words);
    let page = core.search().search(&q(words), &ANYONE, None).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].title, "Alphabet");
}

#[test]
fn expansion_widens_the_facets_and_the_total_with_the_hits() {
    let (_root, core) = open_core("syn_facets");
    let id = asset_id_n(1);
    core.catalog().register_asset(&id, "rik2", NOW).unwrap();
    let mut a = ann("Dog", "dog pet; standalone");
    a.tags = vec!["vlm-cat-character".into()];
    core.search().set_annotation(&id, &a, NOW).unwrap();

    let page = core
        .search()
        .search(&SearchQuery { facets: 4, ..q("puppy") }, &ANYONE, None)
        .unwrap();
    // Count, hits and facets are cut from one snapshot of one candidate set:
    // a synonym hit is a hit everywhere or nowhere.
    assert_eq!(page.total, 1);
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.facets.len(), 1);
    assert_eq!(page.facets[0].label, "vlm-cat-character");
    assert_eq!(page.facets[0].count, 1);
}
