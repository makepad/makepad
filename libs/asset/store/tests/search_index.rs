//! The alias-aware search index: alias terms and canonical-alias ordering,
//! transactional reindex on alias retarget, generation-carrying keyset
//! cursors that fail closed on stale and tampered bytes, and whole-reindex
//! rollback when a statement fails mid-transaction.

mod common;
use common::*;
use makepad_asset_store::{
    AssetAnnotation, SearchFilters, SearchQuery, SearchViewer, ServerError, ViewerScope,
    Visibility,
};
use makepad_asset_data::{AssetId, AssetRevisionRef};

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

fn q(text: &str) -> SearchQuery<'_> {
    SearchQuery { text, filters: SearchFilters::default(), page_size: 10, facets: 0 }
}

fn stale(err: ServerError) {
    assert!(
        matches!(err, ServerError::InvalidInput { what: "stale search cursor" }),
        "expected stale cursor, got {err}"
    );
}

fn malformed(err: ServerError) {
    assert!(
        matches!(err, ServerError::InvalidInput { what: "search cursor malformed" }),
        "expected malformed cursor, got {err}"
    );
}

fn tampered(err: ServerError) {
    assert!(
        matches!(err, ServerError::InvalidInput { what: "search cursor tampered" }),
        "expected tampered cursor, got {err}"
    );
}

#[test]
fn alias_terms_index_and_retarget_reindexes_transactionally() {
    let (_root, core) = open_core("alias_terms");
    let search = core.search();
    let (id_a, _rev_a) = publish_prop(&core, "rik2", 1, b"glb a", b"thumb a", NOW);
    let (id_b, rev_b) = publish_prop(&core, "rik2", 2, b"glb b", b"thumb b", NOW);
    search.set_annotation(&id_a, &ann("lantern alpha"), NOW).unwrap();
    search.set_annotation(&id_b, &ann("lantern beta"), NOW).unwrap();

    // No alias yet: alias-shaped terms match nothing, hits carry no alias.
    assert_eq!(search.search(&q("lamp"), &ANYONE, None).unwrap().total, 0);
    let page = search.search(&q("lantern"), &ANYONE, None).unwrap();
    assert!(page.hits.iter().all(|h| h.alias.is_none()));

    // An alias head indexes every tokenized segment, normalized exactly like
    // annotation text (lowercase runs, `_` and `-` split, digits kept).
    let alias = "rik2/props/old-lamp_3".parse().unwrap();
    let rev_of = |id: AssetId, rev| AssetRevisionRef { asset_id: id, revision: rev };
    core.catalog().set_asset_alias(&alias, &rev_of(id_a, _rev_a), NOW + 1).unwrap();
    for term in ["lamp", "old", "props", "3", "LAMP", "rik2"] {
        let page = search.search(&q(term), &ANYONE, None).unwrap();
        assert_eq!((page.total, page.hits[0].asset_id), (1, id_a), "term {term}");
    }
    let page = search.search(&q("lamp"), &ANYONE, None).unwrap();
    assert_eq!(page.hits[0].alias.as_deref(), Some("rik2/props/old-lamp_3"));
    assert!(page.hits[0].live);
    // Multi-term AND spans annotation and alias postings as one index.
    let page = search.search(&q("lantern lamp"), &ANYONE, None).unwrap();
    assert_eq!((page.total, page.hits[0].asset_id), (1, id_a));
    // At equal score the unaliased asset sorts first ('' before any alias).
    let page = search.search(&q("lantern"), &ANYONE, None).unwrap();
    let got: Vec<(AssetId, Option<&str>)> =
        page.hits.iter().map(|h| (h.asset_id, h.alias.as_deref())).collect();
    assert_eq!(got, vec![(id_b, None), (id_a, Some("rik2/props/old-lamp_3"))]);

    // Retargeting moves the terms, canonical alias and liveness in one
    // transaction: the old target keeps nothing.
    core.catalog().set_asset_alias(&alias, &rev_of(id_b, rev_b), NOW + 2).unwrap();
    let page = search.search(&q("lamp"), &ANYONE, None).unwrap();
    assert_eq!((page.total, page.hits[0].asset_id), (1, id_b));
    let page = search.search(&q("lantern"), &ANYONE, None).unwrap();
    let got: Vec<(AssetId, Option<&str>, bool)> =
        page.hits.iter().map(|h| (h.asset_id, h.alias.as_deref(), h.live)).collect();
    assert_eq!(
        got,
        vec![(id_a, None, false), (id_b, Some("rik2/props/old-lamp_3"), true)]
    );

    // With several heads the canonical alias is the smallest one; clearing it
    // falls back to the next.
    let alias2 = "rik2/props/beta-lamp".parse().unwrap();
    core.catalog().set_asset_alias(&alias2, &rev_of(id_b, rev_b), NOW + 3).unwrap();
    let page = search.search(&q("lamp"), &ANYONE, None).unwrap();
    assert_eq!(page.hits[0].alias.as_deref(), Some("rik2/props/beta-lamp"));
    // "beta" now reaches id_b through BOTH its title (100) and an alias
    // segment (80): one hit, summed across the two posting tables.
    let page = search.search(&q("beta"), &ANYONE, None).unwrap();
    assert_eq!((page.total, page.hits[0].asset_id, page.hits[0].score), (1, id_b, 180));
    assert!(core.catalog().clear_asset_alias(&alias2).unwrap());
    let page = search.search(&q("lamp"), &ANYONE, None).unwrap();
    assert_eq!(page.hits[0].alias.as_deref(), Some("rik2/props/old-lamp_3"));

    // Clearing the last head removes the terms; annotations are untouched.
    assert!(core.catalog().clear_asset_alias(&alias).unwrap());
    assert_eq!(search.search(&q("lamp"), &ANYONE, None).unwrap().total, 0);
    assert_eq!(search.search(&q("lantern"), &ANYONE, None).unwrap().total, 2);

    // Quarantining the aliased revision tears the alias terms down too.
    core.catalog().set_asset_alias(&alias, &rev_of(id_b, rev_b), NOW + 4).unwrap();
    assert_eq!(search.search(&q("lamp"), &ANYONE, None).unwrap().total, 1);
    core.catalog().quarantine_asset(&id_b, &rev_b, NOW + 5).unwrap();
    assert_eq!(search.search(&q("lamp"), &ANYONE, None).unwrap().total, 0);
    assert_eq!(core.catalog().resolve_asset_alias(&alias).unwrap(), None);
}

#[test]
fn canonical_alias_orders_equal_scores_and_pages_without_dups() {
    let (_root, core) = open_core("canon_order");
    let search = core.search();
    // Four equal-score assets: two unaliased, two aliased. The alias segments
    // deliberately avoid the query term so scores stay identical.
    let id1 = asset_id_n(1);
    core.catalog().register_asset(&id1, "rik2", NOW).unwrap();
    let (id2, rev2) = publish_prop(&core, "rik2", 2, b"g2", b"t2", NOW);
    let (id3, rev3) = publish_prop(&core, "rik2", 3, b"g3", b"t3", NOW);
    let id4 = asset_id_n(4);
    core.catalog().register_asset(&id4, "rik2", NOW).unwrap();
    for id in [id1, id2, id3, id4] {
        search.set_annotation(&id, &ann("beacon post"), NOW).unwrap();
    }
    core.catalog()
        .set_asset_alias(
            &"rik2/items/bb".parse().unwrap(),
            &AssetRevisionRef { asset_id: id2, revision: rev2 },
            NOW + 1,
        )
        .unwrap();
    core.catalog()
        .set_asset_alias(
            &"rik2/items/aa".parse().unwrap(),
            &AssetRevisionRef { asset_id: id3, revision: rev3 },
            NOW + 2,
        )
        .unwrap();

    // Total order: score DESC (all equal), canonical alias ASC with ''
    // (unaliased) first, asset id ASC as the last key.
    let page = search.search(&q("beacon"), &ANYONE, None).unwrap();
    let full: Vec<(AssetId, Option<&str>)> =
        page.hits.iter().map(|h| (h.asset_id, h.alias.as_deref())).collect();
    assert_eq!(
        full,
        vec![
            (id1, None),
            (id4, None),
            (id3, Some("rik2/items/aa")),
            (id2, Some("rik2/items/bb")),
        ]
    );
    assert!(page.hits.windows(2).all(|w| w[0].score == w[1].score), "scores tied");

    // A page-size-1 keyset walk over the tied set: no dup, no miss, same
    // order as the unpaged query.
    let mut one = q("beacon");
    one.page_size = 1;
    let mut seen = Vec::new();
    let mut cursor: Option<Vec<u8>> = None;
    loop {
        let page = search.search(&one, &ANYONE, cursor.as_deref()).unwrap();
        assert_eq!(page.total, 4, "total constant across the walk");
        seen.extend(page.hits.iter().map(|h| h.asset_id));
        match page.cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
        assert!(seen.len() <= 4, "walk must terminate");
    }
    assert_eq!(seen, vec![id1, id4, id3, id2]);

    // Browse mode follows the same total order (every score is zero).
    let mut browse = q("");
    browse.filters = SearchFilters { namespace: Some("rik2"), ..Default::default() };
    let page = search.search(&browse, &ANYONE, None).unwrap();
    assert_eq!(
        page.hits.iter().map(|h| h.asset_id).collect::<Vec<_>>(),
        vec![id1, id4, id3, id2]
    );
}

#[test]
fn index_mutations_invalidate_cursors_and_noops_do_not() {
    let (_root, core) = open_core("generation");
    let search = core.search();
    let (id3, rev3) = publish_prop(&core, "rik2", 3, b"g3", b"t3", NOW);
    let id1 = asset_id_n(1);
    let id2 = asset_id_n(2);
    core.catalog().register_asset(&id1, "rik2", NOW).unwrap();
    core.catalog().register_asset(&id2, "rik2", NOW).unwrap();
    let id4 = asset_id_n(4);
    core.catalog().register_asset(&id4, "rik2", NOW).unwrap();
    search.set_annotation(&id1, &ann("gizmo one"), NOW).unwrap();
    search.set_annotation(&id2, &ann("gizmo two"), NOW).unwrap();
    search.set_annotation(&id3, &ann("gizmo three"), NOW).unwrap();

    let mut one = q("gizmo");
    one.page_size = 1;
    let mint = || search.search(&one, &ANYONE, None).unwrap().cursor.expect("more hits");

    // Annotating ANY asset (even one not in this result set) retires the
    // cursor: the total order may have shifted under it.
    let c = mint();
    search.set_annotation(&id2, &ann("gizmo two prime"), NOW + 1).unwrap();
    stale(search.search(&one, &ANYONE, Some(&c)).unwrap_err());

    // Alias-head set retires cursors.
    let c = mint();
    let alias = "rik2/misc/tool".parse().unwrap();
    core.catalog()
        .set_asset_alias(&alias, &AssetRevisionRef { asset_id: id3, revision: rev3 }, NOW + 2)
        .unwrap();
    stale(search.search(&one, &ANYONE, Some(&c)).unwrap_err());

    // Clearing an annotation retires cursors.
    let c = mint();
    search.clear_annotation(&id1).unwrap();
    stale(search.search(&one, &ANYONE, Some(&c)).unwrap_err());

    // Pure no-ops do NOT: clearing a never-annotated asset or a nonexistent
    // alias leaves the generation alone and the cursor keeps working.
    let c = mint();
    search.clear_annotation(&id4).unwrap();
    assert!(!core.catalog().clear_asset_alias(&"rik2/misc/ghost".parse().unwrap()).unwrap());
    let page = search.search(&one, &ANYONE, Some(&c)).unwrap();
    assert_eq!(page.hits.len(), 1);

    // Quarantine that drops an alias head retires cursors.
    core.catalog().quarantine_asset(&id3, &rev3, NOW + 3).unwrap();
    stale(search.search(&one, &ANYONE, Some(&c)).unwrap_err());
}

#[test]
fn cursor_tampering_and_hostile_bytes_fail_closed() {
    let (_root, core) = open_core("tamper");
    let search = core.search();
    for n in 1..=2u8 {
        let id = asset_id_n(n);
        core.catalog().register_asset(&id, "rik2", NOW).unwrap();
        search.set_annotation(&id, &ann(&format!("brass item {n}")), NOW).unwrap();
    }
    let mut one = q("brass");
    one.page_size = 1;
    let cursor = search.search(&one, &ANYONE, None).unwrap().cursor.expect("two hits");

    // Any bit flip inside the checksummed body is tampering: generation,
    // fingerprint, score, asset id and the check itself are all covered.
    for idx in [1usize, 20, 45, cursor.len() - 17, cursor.len() - 1] {
        let mut bad = cursor.clone();
        bad[idx] ^= 1;
        tampered(search.search(&one, &ANYONE, Some(&bad)).unwrap_err());
    }
    // Structural damage is malformed: wrong version, truncation, growth,
    // emptiness, oversize garbage, forged alias length.
    let mut bad = cursor.clone();
    bad[0] = 9;
    malformed(search.search(&one, &ANYONE, Some(&bad)).unwrap_err());
    malformed(search.search(&one, &ANYONE, Some(&cursor[..cursor.len() - 1])).unwrap_err());
    let mut bad = cursor.clone();
    bad.push(0);
    malformed(search.search(&one, &ANYONE, Some(&bad)).unwrap_err());
    malformed(search.search(&one, &ANYONE, Some(b"")).unwrap_err());
    malformed(search.search(&one, &ANYONE, Some(&vec![0u8; 10_000])).unwrap_err());
    let mut bad = cursor.clone();
    bad[49..51].copy_from_slice(&200u16.to_be_bytes());
    malformed(search.search(&one, &ANYONE, Some(&bad)).unwrap_err());
    let mut bad = cursor.clone();
    bad[49..51].copy_from_slice(&5u16.to_be_bytes());
    malformed(search.search(&one, &ANYONE, Some(&bad)).unwrap_err());

    // The genuine cursor still works after all hostile traffic.
    let page = search.search(&one, &ANYONE, Some(&cursor)).unwrap();
    assert_eq!(page.hits.len(), 1);

    // Same laws when the keyset position carries a real alias.
    let (id6, rev6) = publish_prop(&core, "rik2", 6, b"g6", b"t6", NOW);
    let (id7, rev7) = publish_prop(&core, "rik2", 7, b"g7", b"t7", NOW);
    let id5 = asset_id_n(5);
    core.catalog().register_asset(&id5, "rik2", NOW).unwrap();
    for id in [id5, id6, id7] {
        search.set_annotation(&id, &ann("copper beacon"), NOW).unwrap();
    }
    core.catalog()
        .set_asset_alias(
            &"rik2/set/aa".parse().unwrap(),
            &AssetRevisionRef { asset_id: id6, revision: rev6 },
            NOW + 1,
        )
        .unwrap();
    core.catalog()
        .set_asset_alias(
            &"rik2/set/bb".parse().unwrap(),
            &AssetRevisionRef { asset_id: id7, revision: rev7 },
            NOW + 2,
        )
        .unwrap();
    let mut two = q("copper");
    two.page_size = 2;
    let page = search.search(&two, &ANYONE, None).unwrap();
    assert_eq!(
        page.hits.iter().map(|h| h.alias.as_deref()).collect::<Vec<_>>(),
        vec![None, Some("rik2/set/aa")]
    );
    let cursor = page.cursor.expect("third hit remains");
    // Flip a byte inside the alias region.
    let mut bad = cursor.clone();
    bad[55] ^= 1;
    tampered(search.search(&two, &ANYONE, Some(&bad)).unwrap_err());
    // Forge the alias length on an alias-carrying cursor.
    let mut bad = cursor.clone();
    bad[49..51].copy_from_slice(&0u16.to_be_bytes());
    malformed(search.search(&two, &ANYONE, Some(&bad)).unwrap_err());
    let page = search.search(&two, &ANYONE, Some(&cursor)).unwrap();
    assert_eq!(
        page.hits.iter().map(|h| h.asset_id).collect::<Vec<_>>(),
        vec![id7]
    );
}

#[test]
fn mid_transaction_failure_rolls_back_the_whole_reindex() {
    let (root, core) = open_core("rollback");
    let db = root.join("catalog.sqlite3");
    let search = core.search();
    let (id_a, rev_a) = publish_prop(&core, "rik2", 1, b"glb a", b"thumb a", NOW);
    let mut aa = ann("alpha beta");
    aa.tags = vec!["durable".into()];
    search.set_annotation(&id_a, &aa, NOW).unwrap();
    let alias = "rik2/props/lamp".parse().unwrap();
    core.catalog()
        .set_asset_alias(&alias, &AssetRevisionRef { asset_id: id_a, revision: rev_a }, NOW + 1)
        .unwrap();

    // Annotation path: the posting insert fires AFTER the upsert and the
    // delete-old-index statements in the same transaction. Failing it must
    // roll all of them back.
    let g0 = read_generation(&db);
    raw::exec(
        &db,
        "CREATE TRIGGER boom BEFORE INSERT ON search_postings
         BEGIN SELECT RAISE(ABORT, 'boom'); END",
    );
    let err = search.set_annotation(&id_a, &ann("gamma delta"), NOW + 2).unwrap_err();
    assert!(matches!(err, ServerError::Db { .. }), "{err}");
    assert_eq!(read_generation(&db), g0, "failed tx must not advance the generation");
    assert_eq!(search.annotation(&id_a).unwrap().unwrap().title, "alpha beta");
    assert_eq!(search.search(&q("alpha"), &ANYONE, None).unwrap().total, 1);
    assert_eq!(search.search(&q("gamma"), &ANYONE, None).unwrap().total, 0);
    let mut tagged = q("");
    tagged.filters = SearchFilters { tag: Some("durable"), ..Default::default() };
    assert_eq!(search.search(&tagged, &ANYONE, None).unwrap().total, 1);
    let page = search.search(&q("lamp"), &ANYONE, None).unwrap();
    assert_eq!((page.total, page.hits[0].live), (1, true), "alias index intact");

    // With the fault removed the identical write lands, advancing the
    // generation exactly once; alias postings survive an annotation rebuild.
    raw::exec(&db, "DROP TRIGGER boom");
    search.set_annotation(&id_a, &ann("gamma delta"), NOW + 3).unwrap();
    assert_eq!(read_generation(&db), g0 + 1);
    assert_eq!(search.search(&q("gamma"), &ANYONE, None).unwrap().total, 1);
    assert_eq!(search.search(&q("alpha"), &ANYONE, None).unwrap().total, 0);
    assert_eq!(search.search(&q("lamp"), &ANYONE, None).unwrap().total, 1);

    // Alias path: failing the alias-posting rebuild rolls back the CATALOG
    // alias write too — the head, the live flag, the canonical alias, the
    // terms and the generation all stay exactly as before.
    let (id_b, rev_b) = publish_prop(&core, "rik2", 2, b"glb b", b"thumb b", NOW);
    search.set_annotation(&id_b, &ann("omega"), NOW + 4).unwrap();
    let g1 = read_generation(&db);
    raw::exec(
        &db,
        "CREATE TRIGGER boom2 BEFORE INSERT ON search_alias_postings
         BEGIN SELECT RAISE(ABORT, 'boom'); END",
    );
    let alias_b = "rik2/props/blamp".parse().unwrap();
    let err = core
        .catalog()
        .set_asset_alias(&alias_b, &AssetRevisionRef { asset_id: id_b, revision: rev_b }, NOW + 5)
        .unwrap_err();
    assert!(matches!(err, ServerError::Db { .. }), "{err}");
    assert_eq!(core.catalog().resolve_asset_alias(&alias_b).unwrap(), None);
    assert_eq!(read_generation(&db), g1);
    assert_eq!(search.search(&q("blamp"), &ANYONE, None).unwrap().total, 0);
    let page = search.search(&q("omega"), &ANYONE, None).unwrap();
    assert_eq!((page.hits[0].live, page.hits[0].alias.as_deref()), (false, None));

    raw::exec(&db, "DROP TRIGGER boom2");
    core.catalog()
        .set_asset_alias(&alias_b, &AssetRevisionRef { asset_id: id_b, revision: rev_b }, NOW + 6)
        .unwrap();
    assert_eq!(read_generation(&db), g1 + 1);
    let page = search.search(&q("blamp"), &ANYONE, None).unwrap();
    assert_eq!(
        (page.total, page.hits[0].live, page.hits[0].alias.as_deref()),
        (1, true, Some("rik2/props/blamp"))
    );
}
