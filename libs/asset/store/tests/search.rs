//! Catalog search: annotation bounds, deterministic ranking, exact filters,
//! cursor pagination, restart persistence, transactional reindex, and the
//! authorization scope that keeps prompts out of hits/snippets/counts.

mod common;
use common::*;
use makepad_asset_store::{
    AssetAnnotation, AssetServerCore, Budgets, SearchFilters, SearchQuery, SearchViewer,
    ServerError, ViewerScope, Visibility,
};
use makepad_asset_data::{AssetId, AssetKind, AssetRevisionRef};

const ANYONE: SearchViewer<'static> = SearchViewer { principal: None, scope: ViewerScope::All };

fn ann(title: &str) -> AssetAnnotation {
    AssetAnnotation {
        title: title.into(),
        description: String::new(),
        kind: None,
        categories: Vec::new(),
        tags: Vec::new(),
        creator: String::new(),
        owner: None,
        generator: String::new(),
        backend: String::new(),
        model: String::new(),
        prompt: String::new(),
        provenance: String::new(),
        visibility: Visibility::Public,
    }
}

fn reg(core: &AssetServerCore, n: u8, ns: &str) -> AssetId {
    let id = asset_id_n(n);
    core.catalog().register_asset(&id, ns, NOW).unwrap();
    id
}

fn q<'a>(text: &'a str) -> SearchQuery<'a> {
    SearchQuery { text, filters: SearchFilters::default(), page_size: 10 }
}

#[test]
fn annotation_bounds_are_fail_closed() {
    let (_root, core) = open_core("ann_bounds");
    let search = core.search();

    // Unregistered asset refuses.
    assert!(matches!(
        search.set_annotation(&asset_id_n(9), &ann("ghost"), NOW).unwrap_err(),
        ServerError::NotFound { what: "asset for annotation" }
    ));
    let id = reg(&core, 1, "rik2");

    // Empty and oversize titles refuse.
    assert!(matches!(
        search.set_annotation(&id, &ann(""), NOW).unwrap_err(),
        ServerError::InvalidInput { what: "annotation title empty" }
    ));
    assert!(matches!(
        search.set_annotation(&id, &ann(&"x".repeat(201)), NOW).unwrap_err(),
        ServerError::OverBudget { what: "annotation title bytes", .. }
    ));
    // Control characters in single-line fields refuse.
    assert!(matches!(
        search.set_annotation(&id, &ann("evil\x1b[31mtitle"), NOW).unwrap_err(),
        ServerError::InvalidInput { what: "annotation title charset" }
    ));
    // Oversize prompt refuses.
    let mut a = ann("ok");
    a.prompt = "p".repeat(8193);
    assert!(matches!(
        search.set_annotation(&id, &a, NOW).unwrap_err(),
        ServerError::OverBudget { what: "annotation prompt bytes", .. }
    ));
    // Too many tags / bad label charset refuse.
    let mut a = ann("ok");
    a.tags = (0..25).map(|i| format!("t{i}")).collect();
    assert!(matches!(
        search.set_annotation(&id, &a, NOW).unwrap_err(),
        ServerError::OverBudget { what: "annotation tags", .. }
    ));
    let mut a = ann("ok");
    a.tags = vec!["Bad Tag".into()];
    assert!(matches!(
        search.set_annotation(&id, &a, NOW).unwrap_err(),
        ServerError::InvalidInput { what: "annotation label charset" }
    ));
    // Private without an owner refuses.
    let mut a = ann("ok");
    a.visibility = Visibility::Private;
    assert!(matches!(
        search.set_annotation(&id, &a, NOW).unwrap_err(),
        ServerError::InvalidInput { what: "private annotation requires owner" }
    ));

    // A valid annotation lands and round-trips, labels sorted+deduped.
    let mut a = ann("Crystal Sword");
    a.tags = vec!["weapon".into(), "crystal".into(), "weapon".into()];
    a.description = "A fine blade".into();
    search.set_annotation(&id, &a, NOW).unwrap();
    let read = search.annotation(&id).unwrap().unwrap();
    assert_eq!(read.title, "Crystal Sword");
    assert_eq!(read.tags, vec!["crystal".to_string(), "weapon".to_string()]);
    assert!(search.annotation(&asset_id_n(2)).unwrap().is_none());
}

#[test]
fn query_bounds_and_cursors_are_checked() {
    let (_root, core) = open_core("query_bounds");
    let search = core.search();
    let id = reg(&core, 1, "rik2");
    search.set_annotation(&id, &ann("alpha beta"), NOW).unwrap();

    // Hostile query shapes refuse.
    let big = "x".repeat(1025);
    assert!(matches!(
        search.search(&q(&big), &ANYONE, None).unwrap_err(),
        ServerError::OverBudget { what: "search query bytes", .. }
    ));
    let many: String = (0..33).map(|i| format!("t{i} ")).collect();
    assert!(matches!(
        search.search(&q(&many), &ANYONE, None).unwrap_err(),
        ServerError::OverBudget { what: "search query terms", .. }
    ));
    assert!(matches!(
        search.search(&q("!!! ???"), &ANYONE, None).unwrap_err(),
        ServerError::InvalidInput { what: "search query has no terms" }
    ));
    let mut zq = q("alpha");
    zq.page_size = 0;
    assert!(matches!(
        search.search(&zq, &ANYONE, None).unwrap_err(),
        ServerError::InvalidInput { what: "search page size zero" }
    ));
    zq.page_size = 101;
    assert!(matches!(
        search.search(&zq, &ANYONE, None).unwrap_err(),
        ServerError::OverBudget { what: "search page size", .. }
    ));

    // Cursors: malformed bytes, wrong version, and cross-query replay refuse.
    assert!(matches!(
        search.search(&q("alpha"), &ANYONE, Some(b"junk")).unwrap_err(),
        ServerError::InvalidInput { what: "search cursor malformed" }
    ));
    let page = search.search(&q("alpha"), &ANYONE, None).unwrap();
    assert_eq!(page.hits.len(), 1);
    assert!(page.cursor.is_none(), "single page needs no cursor");
    // Force a cursor by page_size 1 over two matching assets.
    let id2 = reg(&core, 2, "rik2");
    search.set_annotation(&id2, &ann("alpha gamma"), NOW).unwrap();
    let mut one = q("alpha");
    one.page_size = 1;
    let page = search.search(&one, &ANYONE, None).unwrap();
    let cursor = page.cursor.clone().expect("more results exist");
    // Same cursor against a different query text is stale.
    let mut other = q("gamma");
    other.page_size = 1;
    assert!(matches!(
        search.search(&other, &ANYONE, Some(&cursor)).unwrap_err(),
        ServerError::InvalidInput { what: "stale search cursor" }
    ));
    // Corrupt version byte is malformed.
    let mut bad = cursor.clone();
    bad[0] = 9;
    assert!(matches!(
        search.search(&one, &ANYONE, Some(&bad)).unwrap_err(),
        ServerError::InvalidInput { what: "search cursor malformed" }
    ));
    // The genuine cursor continues.
    let page2 = search.search(&one, &ANYONE, Some(&cursor)).unwrap();
    assert_eq!(page2.hits.len(), 1);
    assert!(page2.cursor.is_none());
}

#[test]
fn ranking_is_field_weighted_and_deterministic() {
    let (_root, core) = open_core("ranking");
    let search = core.search();
    let a = reg(&core, 1, "rik2");
    let b = reg(&core, 2, "rik2");
    let c = reg(&core, 3, "rik2");
    // Same term in three fields of different weight.
    search.set_annotation(&a, &ann("crystal sword"), NOW).unwrap();
    let mut bd = ann("plain thing");
    bd.description = "a crystal shard".into();
    search.set_annotation(&b, &bd, NOW).unwrap();
    let mut cd = ann("other thing");
    cd.tags = vec!["crystal".into()];
    search.set_annotation(&c, &cd, NOW).unwrap();

    let page = search.search(&q("crystal"), &ANYONE, None).unwrap();
    assert_eq!(page.total, 3);
    let order: Vec<AssetId> = page.hits.iter().map(|h| h.asset_id).collect();
    assert_eq!(order, vec![a, c, b], "title > tag > description weight");
    assert!(page.hits[0].score > page.hits[1].score);
    assert!(page.hits[1].score > page.hits[2].score);

    // Multi-term queries are AND: only the asset with both terms matches.
    let page = search.search(&q("crystal sword"), &ANYONE, None).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].asset_id, a);

    // Equal scores tie-break by asset_id ascending.
    let d = reg(&core, 4, "rik2");
    search.set_annotation(&d, &ann("crystal sword"), NOW).unwrap();
    let page = search.search(&q("sword"), &ANYONE, None).unwrap();
    assert_eq!(
        page.hits.iter().map(|h| h.asset_id).collect::<Vec<_>>(),
        vec![a, d]
    );
}

#[test]
fn filters_are_exact_and_composable() {
    let (_root, core) = open_core("filters");
    let search = core.search();
    let a = reg(&core, 1, "rik2");
    let b = reg(&core, 2, "rik2");
    let c = reg(&core, 3, "other");
    let mut aa = ann("red robot");
    aa.tags = vec!["mech".into()];
    aa.categories = vec!["props".into()];
    aa.creator = "rik".into();
    aa.model = "flux-2".into();
    search.set_annotation(&a, &aa, NOW).unwrap();
    let mut ba = ann("blue robot");
    ba.tags = vec!["organic".into()];
    ba.creator = "ana".into();
    search.set_annotation(&b, &ba, NOW).unwrap();
    search.set_annotation(&c, &ann("green robot"), NOW).unwrap();

    let hits = |filters: SearchFilters| {
        let mut query = q("robot");
        query.filters = filters;
        let page = search.search(&query, &ANYONE, None).unwrap();
        (page.hits.iter().map(|h| h.asset_id).collect::<Vec<_>>(), page.total)
    };
    assert_eq!(hits(SearchFilters { tag: Some("mech"), ..Default::default() }), (vec![a], 1));
    assert_eq!(hits(SearchFilters { category: Some("props"), ..Default::default() }), (vec![a], 1));
    assert_eq!(hits(SearchFilters { creator: Some("ana"), ..Default::default() }), (vec![b], 1));
    assert_eq!(hits(SearchFilters { model: Some("flux-2"), ..Default::default() }), (vec![a], 1));
    assert_eq!(hits(SearchFilters { namespace: Some("other"), ..Default::default() }), (vec![c], 1));
    // Composed filters intersect; a non-matching composition is empty.
    assert_eq!(
        hits(SearchFilters { creator: Some("ana"), tag: Some("mech"), ..Default::default() }),
        (vec![], 0)
    );
    // Hostile filter values refuse.
    let mut query = q("robot");
    query.filters = SearchFilters { tag: Some("Bad Tag"), ..Default::default() };
    assert!(matches!(
        search.search(&query, &ANYONE, None).unwrap_err(),
        ServerError::InvalidInput { what: "annotation label charset" }
    ));
    query.filters = SearchFilters { creator: Some(""), ..Default::default() };
    assert!(matches!(
        search.search(&query, &ANYONE, None).unwrap_err(),
        ServerError::InvalidInput { what: "search filter empty" }
    ));
}

#[test]
fn pagination_is_stable_without_dup_or_miss() {
    let (_root, core) = open_core("pagination");
    let search = core.search();
    for n in 1..=5u8 {
        let id = reg(&core, n, "rik2");
        // Different weights: title match for some, description for others.
        let mut a = if n % 2 == 0 { ann(&format!("gadget {n}")) } else { ann(&format!("thing {n}")) };
        if n % 2 != 0 {
            a.description = format!("a gadget numbered {n}");
        }
        search.set_annotation(&id, &a, NOW).unwrap();
    }
    let mut query = q("gadget");
    query.page_size = 2;
    let mut seen = Vec::new();
    let mut cursor: Option<Vec<u8>> = None;
    let mut pages = 0;
    loop {
        let page = search.search(&query, &ANYONE, cursor.as_deref()).unwrap();
        assert_eq!(page.total, 5, "total is constant on every page");
        seen.extend(page.hits.iter().map(|h| h.asset_id));
        pages += 1;
        match page.cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
        assert!(pages < 10, "pagination must terminate");
    }
    assert_eq!(pages, 3, "5 hits at page_size 2 = 2+2+1");
    assert_eq!(seen.len(), 5, "no dup, no miss");
    let mut dedup = seen.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), 5);
    // Scores never increase across the walk.
    let full = search.search(&q("gadget"), &ANYONE, None).unwrap();
    assert_eq!(full.hits.iter().map(|h| h.asset_id).collect::<Vec<_>>(), seen);

    // Browse mode (no terms) pages by asset_id with filters only.
    let mut browse = q("");
    browse.page_size = 2;
    browse.filters = SearchFilters { namespace: Some("rik2"), ..Default::default() };
    let mut seen = Vec::new();
    let mut cursor: Option<Vec<u8>> = None;
    loop {
        let page = search.search(&browse, &ANYONE, cursor.as_deref()).unwrap();
        assert_eq!(page.total, 5);
        seen.extend(page.hits.iter().map(|h| h.asset_id));
        match page.cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, (1..=5u8).map(asset_id_n).collect::<Vec<_>>());
}

#[test]
fn search_survives_restart() {
    let root = test_root("search_restart");
    let id;
    {
        let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
        id = reg(&core, 1, "rik2");
        let mut a = ann("Persistent Artifact");
        a.tags = vec!["durable".into()];
        core.search().set_annotation(&id, &a, NOW).unwrap();
    }
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    core.recover(NOW + 10).unwrap();
    let page = core.search().search(&q("persistent"), &ANYONE, None).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].asset_id, id);
    let mut fq = q("");
    fq.filters = SearchFilters { tag: Some("durable"), ..Default::default() };
    assert_eq!(core.search().search(&fq, &ANYONE, None).unwrap().total, 1);
}

#[test]
fn mutation_reindexes_transactionally() {
    let (_root, core) = open_core("reindex");
    let search = core.search();
    let id = reg(&core, 1, "rik2");
    search.set_annotation(&id, &ann("obsidian golem"), NOW).unwrap();
    assert_eq!(search.search(&q("obsidian"), &ANYONE, None).unwrap().total, 1);

    // Replacing the annotation fully supersedes the old postings.
    search.set_annotation(&id, &ann("marble statue"), NOW + 1).unwrap();
    assert_eq!(search.search(&q("obsidian"), &ANYONE, None).unwrap().total, 0);
    assert_eq!(search.search(&q("marble"), &ANYONE, None).unwrap().total, 1);

    // Clearing removes the asset from hits and counts.
    search.clear_annotation(&id).unwrap();
    search.clear_annotation(&id).unwrap(); // idempotent
    assert_eq!(search.search(&q("marble"), &ANYONE, None).unwrap().total, 0);
    assert!(search.annotation(&id).unwrap().is_none());
}

#[test]
fn alias_head_mutation_updates_liveness() {
    let (_root, core) = open_core("liveness");
    let search = core.search();
    let (id_a, rev_a) = publish_prop(&core, "rik2", 1, b"glb a", b"thumb a", NOW);
    let (id_b, rev_b) = publish_prop(&core, "rik2", 2, b"glb b", b"thumb b", NOW);
    search.set_annotation(&id_a, &ann("lantern alpha"), NOW).unwrap();
    search.set_annotation(&id_b, &ann("lantern beta"), NOW).unwrap();

    let live = |term: &str| {
        let mut query = q(term);
        query.filters = SearchFilters { live_only: true, ..Default::default() };
        search.search(&query, &ANYONE, None).unwrap()
    };
    // No alias yet: nothing is live (hits and counts agree).
    assert_eq!(live("lantern").total, 0);

    let alias = "rik2/props/lamp".parse().unwrap();
    core.catalog()
        .set_asset_alias(&alias, &AssetRevisionRef { asset_id: id_a, revision: rev_a }, NOW + 1)
        .unwrap();
    let page = live("lantern");
    assert_eq!((page.total, page.hits[0].asset_id, page.hits[0].live), (1, id_a, true));

    // Retargeting the only alias to B flips both flags in one transaction.
    core.catalog()
        .set_asset_alias(&alias, &AssetRevisionRef { asset_id: id_b, revision: rev_b }, NOW + 2)
        .unwrap();
    let page = live("lantern");
    assert_eq!((page.total, page.hits[0].asset_id), (1, id_b));
    // Unfiltered search still sees both, with truthful live flags.
    let page = search.search(&q("lantern"), &ANYONE, None).unwrap();
    let flags: Vec<(AssetId, bool)> = page.hits.iter().map(|h| (h.asset_id, h.live)).collect();
    assert_eq!(flags, vec![(id_a, false), (id_b, true)]);

    // Quarantining the live head drops the alias and the liveness together.
    core.catalog().quarantine_asset(&id_b, &rev_b, NOW + 3).unwrap();
    assert_eq!(core.catalog().resolve_asset_alias(&alias).unwrap(), None);
    assert_eq!(live("lantern").total, 0);

    // Clearing an alias flips liveness in the same transaction too.
    core.catalog()
        .set_asset_alias(&alias, &AssetRevisionRef { asset_id: id_a, revision: rev_a }, NOW + 4)
        .unwrap();
    assert_eq!(live("lantern").total, 1);
    assert!(core.catalog().clear_asset_alias(&alias).unwrap());
    assert!(!core.catalog().clear_asset_alias(&alias).unwrap(), "second clear is a no-op");
    assert_eq!(live("lantern").total, 0);
    // The annotations themselves survive every head mutation.
    assert_eq!(search.search(&q("lantern"), &ANYONE, None).unwrap().total, 2);
}

#[test]
fn authorization_scope_constrains_hits_snippets_and_counts() {
    let (_root, core) = open_core("authz");
    let search = core.search();
    let owner = pid_n(9);
    let stranger = pid_n(8);
    let as_owner = SearchViewer { principal: Some(owner), scope: ViewerScope::All };
    let as_stranger = SearchViewer { principal: Some(stranger), scope: ViewerScope::All };

    // Private annotation: invisible to anonymous and other principals in
    // hits AND totals; visible to its owner.
    let secret = reg(&core, 1, "rik2");
    let mut a = ann("hidden reactor");
    a.visibility = Visibility::Private;
    a.owner = Some(owner);
    search.set_annotation(&secret, &a, NOW).unwrap();
    assert_eq!(search.search(&q("reactor"), &ANYONE, None).unwrap().total, 0);
    assert_eq!(search.search(&q("reactor"), &as_stranger, None).unwrap().total, 0);
    let page = search.search(&q("reactor"), &as_owner, None).unwrap();
    assert_eq!((page.total, page.hits.len()), (1, 1));

    // Prompt terms on a PUBLIC annotation match only for the owner: for
    // everyone else the prompt contributes zero to hits, ranking and counts.
    let pub_id = reg(&core, 2, "rik2");
    let mut a = ann("ordinary vase");
    a.owner = Some(owner);
    a.prompt = "xylograph ceremonial urn in moonlight".into();
    a.description = "a simple container".into();
    search.set_annotation(&pub_id, &a, NOW).unwrap();
    assert_eq!(search.search(&q("xylograph"), &ANYONE, None).unwrap().total, 0);
    assert_eq!(search.search(&q("xylograph"), &as_stranger, None).unwrap().total, 0);
    let page = search.search(&q("xylograph"), &as_owner, None).unwrap();
    assert_eq!(page.total, 1);
    // Even the owner's snippet is built from title/description only.
    assert!(!page.hits[0].snippet.contains("xylograph"), "{}", page.hits[0].snippet);
    assert!(!page.hits[0].snippet.contains("moonlight"));
    // The public fields still match for everyone.
    assert_eq!(search.search(&q("vase"), &ANYONE, None).unwrap().total, 1);

    // Namespace scoping constrains hits and counts.
    let foreign = reg(&core, 3, "other");
    search.set_annotation(&foreign, &ann("ordinary crate"), NOW).unwrap();
    let ns = ["rik2"];
    let scoped = SearchViewer { principal: None, scope: ViewerScope::Namespaces(&ns) };
    assert_eq!(search.search(&q("ordinary"), &ANYONE, None).unwrap().total, 2);
    let page = search.search(&q("ordinary"), &scoped, None).unwrap();
    assert_eq!((page.total, page.hits.len()), (1, 1));
    assert_eq!(page.hits[0].asset_id, pub_id);
    // Zero allowed namespaces sees nothing at all.
    let nothing = SearchViewer { principal: None, scope: ViewerScope::Namespaces(&[]) };
    let page = search.search(&q("ordinary"), &nothing, None).unwrap();
    assert_eq!((page.total, page.hits.len()), (0, 0));
    // Owner filter never bypasses visibility: a stranger filtering by the
    // owner's principal still cannot see the private annotation.
    let mut query = q("reactor");
    query.filters = SearchFilters { owner: Some(owner), ..Default::default() };
    assert_eq!(search.search(&query, &as_stranger, None).unwrap().total, 0);
    assert_eq!(search.search(&query, &as_owner, None).unwrap().total, 1);
}

#[test]
fn kind_is_typed_filterable_and_searchable() {
    let (_root, core) = open_core("kind");
    let search = core.search();
    let a = reg(&core, 1, "rik2");
    let b = reg(&core, 2, "rik2");
    let c = reg(&core, 3, "rik2");
    let mut aa = ann("stone bench");
    aa.kind = Some(AssetKind::Prop);
    aa.tags = vec!["garden".into()];
    search.set_annotation(&a, &aa, NOW).unwrap();
    let mut ba = ann("farm cart");
    ba.kind = Some(AssetKind::Vehicle);
    search.set_annotation(&b, &ba, NOW).unwrap();
    // c declares no kind at all.
    search.set_annotation(&c, &ann("mystery item"), NOW).unwrap();

    // Exact browse filter: only the matching kind; unspecified rows match no
    // kind filter, ever.
    let by_kind = |kind| {
        let mut query = q("");
        query.filters = SearchFilters { kind: Some(kind), ..Default::default() };
        let page = search.search(&query, &ANYONE, None).unwrap();
        (page.hits.iter().map(|h| h.asset_id).collect::<Vec<_>>(), page.total)
    };
    assert_eq!(by_kind(AssetKind::Prop), (vec![a], 1));
    assert_eq!(by_kind(AssetKind::Vehicle), (vec![b], 1));
    assert_eq!(by_kind(AssetKind::Audio), (vec![], 0));

    // The kind name is also a public lexical term.
    let page = search.search(&q("vehicle"), &ANYONE, None).unwrap();
    assert_eq!((page.total, page.hits[0].asset_id), (1, b));
    // Hits carry the typed kind; unspecified rows carry None.
    assert_eq!(page.hits[0].kind, Some(AssetKind::Vehicle));
    let page = search.search(&q("mystery"), &ANYONE, None).unwrap();
    assert_eq!(page.hits[0].kind, None);

    // Full-fidelity read round-trips the kind.
    assert_eq!(search.annotation(&a).unwrap().unwrap().kind, Some(AssetKind::Prop));
    assert_eq!(search.annotation(&c).unwrap().unwrap().kind, None);

    // Kind composes with the other exact filters.
    let mut query = q("");
    query.filters =
        SearchFilters { kind: Some(AssetKind::Prop), tag: Some("garden"), ..Default::default() };
    assert_eq!(search.search(&query, &ANYONE, None).unwrap().total, 1);
    query.filters =
        SearchFilters { kind: Some(AssetKind::Vehicle), tag: Some("garden"), ..Default::default() };
    assert_eq!(search.search(&query, &ANYONE, None).unwrap().total, 0);

    // The kind filter is part of the cursor fingerprint: a cursor minted
    // under it is stale against the same text without it.
    let d = reg(&core, 4, "rik2");
    let mut da = ann("stone table");
    da.kind = Some(AssetKind::Prop);
    search.set_annotation(&d, &da, NOW).unwrap();
    let mut paged = q("stone");
    paged.page_size = 1;
    paged.filters = SearchFilters { kind: Some(AssetKind::Prop), ..Default::default() };
    let cursor = search.search(&paged, &ANYONE, None).unwrap().cursor.expect("two prop hits");
    let mut unfiltered = q("stone");
    unfiltered.page_size = 1;
    assert!(matches!(
        search.search(&unfiltered, &ANYONE, Some(&cursor)).unwrap_err(),
        ServerError::InvalidInput { what: "stale search cursor" }
    ));
    // Against the identical shape the cursor continues.
    let page2 = search.search(&paged, &ANYONE, Some(&cursor)).unwrap();
    assert_eq!((page2.hits.len(), page2.hits[0].asset_id), (1, d));
}

#[test]
fn search_index_terms_budget_is_enforced_before_any_write() {
    let root = test_root("index_terms_budget");
    let budgets = Budgets { max_search_index_terms: 8, ..Budgets::default_v1() };
    let core = AssetServerCore::open(&root, budgets).unwrap();
    let search = core.search();
    let id = reg(&core, 1, "rik2");
    search.set_annotation(&id, &ann("alpha beta"), NOW).unwrap();

    // Nine distinct terms against a budget of eight refuses…
    let err = search
        .set_annotation(&id, &ann("one two three four five six seven eight nine"), NOW + 1)
        .unwrap_err();
    assert!(
        matches!(err, ServerError::OverBudget { what: "search index terms", limit: 8, found: 9 }),
        "{err}"
    );
    // …and the previous annotation is fully intact: the refusal happened
    // before any index row was touched.
    assert_eq!(search.annotation(&id).unwrap().unwrap().title, "alpha beta");
    assert_eq!(search.search(&q("alpha"), &ANYONE, None).unwrap().total, 1);
    assert_eq!(search.search(&q("one"), &ANYONE, None).unwrap().total, 0);

    // Exactly at the budget is admitted.
    search.set_annotation(&id, &ann("one two three four five six seven eight"), NOW + 2).unwrap();
    assert_eq!(search.search(&q("eight"), &ANYONE, None).unwrap().total, 1);
}

#[test]
fn hostile_text_stays_data_never_sql_or_wildcards() {
    let (_root, core) = open_core("hostile");
    let search = core.search();
    let id = reg(&core, 1, "rik2");
    let mut a = ann("Bob's \"quoted\" 100% _thing_ \\ backslash");
    a.creator = "50%_done'; DROP TABLE search_annotations; --".into();
    search.set_annotation(&id, &a, NOW).unwrap();

    // Injection-shaped query text is nothing but terms — and those terms FIND
    // the annotation whose creator contains the same words, proving the
    // hostile string went in as data, not as SQL.
    let page = search.search(&q("'; DROP TABLE search_annotations; --"), &ANYONE, None).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].asset_id, id);

    // Filters are equality over bound literals: no quoting escape, no LIKE
    // wildcard semantics for % or _.
    let mut fq = q("");
    fq.filters = SearchFilters {
        creator: Some("50%_done'; DROP TABLE search_annotations; --"),
        ..Default::default()
    };
    assert_eq!(search.search(&fq, &ANYONE, None).unwrap().total, 1, "exact literal matches");
    fq.filters = SearchFilters { creator: Some("50%"), ..Default::default() };
    assert_eq!(search.search(&fq, &ANYONE, None).unwrap().total, 0, "% is not a wildcard");
    fq.filters = SearchFilters { creator: Some("50%_done"), ..Default::default() };
    assert_eq!(search.search(&fq, &ANYONE, None).unwrap().total, 0, "_ is not a wildcard");
    fq.filters = SearchFilters { creator: Some("x' OR '1'='1"), ..Default::default() };
    assert_eq!(search.search(&fq, &ANYONE, None).unwrap().total, 0, "quotes stay data");

    // Everything still works after the hostile traffic: the tables survived.
    let id2 = reg(&core, 2, "rik2");
    search.set_annotation(&id2, &ann("still standing"), NOW + 1).unwrap();
    assert_eq!(search.search(&q("standing"), &ANYONE, None).unwrap().total, 1);

    // Overlong words truncate identically at index and query time.
    let long = "q".repeat(40);
    let id3 = reg(&core, 3, "rik2");
    search.set_annotation(&id3, &ann(&format!("prefix {long} suffix")), NOW + 2).unwrap();
    let page = search.search(&q(&long), &ANYONE, None).unwrap();
    assert_eq!((page.total, page.hits[0].asset_id), (1, id3));
}

#[test]
fn unicode_survives_admission_snippets_and_search() {
    let (_root, core) = open_core("unicode");
    let search = core.search();
    let id = reg(&core, 1, "rik2");
    let mut a = ann("Übersicht 現在図 sparkle");
    // Multibyte padding forces the snippet window to cut inside non-ASCII
    // text on both sides of the matched term.
    a.description = format!("{} needle {}", "über現在ø ".repeat(40), "śpäß例 ".repeat(40));
    search.set_annotation(&id, &a, NOW).unwrap();
    let page = search.search(&q("needle"), &ANYONE, None).unwrap();
    let snippet = &page.hits[0].snippet;
    assert!(snippet.contains("needle"), "{snippet}");
    assert!(snippet.len() <= 320, "byte-bounded at char boundaries: {}", snippet.len());
    // Terms are ASCII runs: the ASCII tail of a mixed word is indexed, and
    // querying it finds the annotation.
    assert_eq!(search.search(&q("bersicht"), &ANYONE, None).unwrap().total, 1);
}

#[test]
fn snippets_are_normalized_bounded_and_term_centered() {
    let (_root, core) = open_core("snippets");
    let search = core.search();
    let id = reg(&core, 1, "rik2");
    // Escape bytes are refused at admission — they can never even reach a
    // snippet. Legal whitespace (\n \t \r) is admitted and normalized out.
    let mut a = ann("Console Log");
    a.description = "sneaky \x1b[31mred\x1b[0m text".into();
    assert!(matches!(
        search.set_annotation(&id, &a, NOW).unwrap_err(),
        ServerError::InvalidInput { what: "annotation description charset" }
    ));
    let mut a = ann("Console Log");
    a.description = format!(
        "start red\ttext\nwith   gaps {} needle here and a tail",
        "padding word ".repeat(40)
    );
    search.set_annotation(&id, &a, NOW).unwrap();
    let page = search.search(&q("needle"), &ANYONE, None).unwrap();
    let snippet = &page.hits[0].snippet;
    assert!(snippet.contains("needle"), "{snippet}");
    assert!(snippet.len() <= 320, "snippet bounded: {}", snippet.len());
    assert!(!snippet.chars().any(|c| c.is_control()), "control bytes stripped");
    assert!(!snippet.contains("  "), "whitespace collapsed");
    // A hit matched via title has a snippet from the description head.
    let page = search.search(&q("console"), &ANYONE, None).unwrap();
    assert!(page.hits[0].snippet.starts_with("start red text"), "{}", page.hits[0].snippet);
}
