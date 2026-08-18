//! Stock seed behavior: deterministic identities, idempotent re-apply,
//! refusal of non-derived IDs.

mod common;
use common::*;
use makepad_asset_store::{stock_asset_id, SeedAsset, ServerError, StockSeedSource};
use makepad_asset_data::{AssetAlias, BlobId};

struct TestSeed {
    /// When set, the first asset's manifest carries this wrong id instead of
    /// the derived one.
    corrupt_first_id: bool,
}

const ALIAS_A: &str = "stock/props/crate";
const ALIAS_B: &str = "stock/props/barrel";

fn seed_blobs(tag: &str) -> (Vec<u8>, Vec<u8>) {
    (
        format!("stock glb bytes for {tag}").into_bytes(),
        format!("stock thumb bytes for {tag}").into_bytes(),
    )
}

impl StockSeedSource for TestSeed {
    fn name(&self) -> &'static str {
        "test-seed"
    }
    fn assets(&self) -> Vec<SeedAsset> {
        // Deliberately unsorted (B before A): the server sorts by alias.
        [ALIAS_B, ALIAS_A]
            .iter()
            .enumerate()
            .map(|(i, alias)| {
                let alias: AssetAlias = alias.parse().unwrap();
                let (glb, thumb) = seed_blobs(alias.as_str());
                let mut id = stock_asset_id(&alias);
                if self.corrupt_first_id && i == 0 {
                    id = asset_id_n(0xEE);
                }
                SeedAsset {
                    manifest: prop_manifest(id, &glb, &thumb),
                    blobs: vec![glb, thumb],
                    alias,
                }
            })
            .collect()
    }
}

#[test]
fn seed_applies_deterministically_and_idempotently() {
    let (_root, core) = open_core("seed");
    let seed = TestSeed { corrupt_first_id: false };

    let first = core.apply_stock_seed(&seed, NOW).unwrap();
    assert_eq!(first.assets_seen, 2);
    assert_eq!(first.assets_published_new, 2);
    assert_eq!(first.assets_already_published, 0);
    assert_eq!(first.blobs_written, 4);
    assert_eq!(first.blobs_deduped, 0);

    let alias_a: AssetAlias = ALIAS_A.parse().unwrap();
    let alias_b: AssetAlias = ALIAS_B.parse().unwrap();
    let head_a = core.catalog().resolve_asset_alias(&alias_a).unwrap().unwrap();
    let head_b = core.catalog().resolve_asset_alias(&alias_b).unwrap().unwrap();
    // Identities derive from the alias, not from any state.
    assert_eq!(head_a.asset_id, stock_asset_id(&alias_a));
    assert_eq!(head_b.asset_id, stock_asset_id(&alias_b));
    // And the revision is the digest of the manifest bytes the seed produced.
    let (glb, thumb) = seed_blobs(alias_a.as_str());
    let expect_rev = prop_manifest(head_a.asset_id, &glb, &thumb).revision().unwrap();
    assert_eq!(head_a.revision, expect_rev);

    // Second apply: nothing new anywhere, same heads.
    let second = core.apply_stock_seed(&seed, NOW + 10).unwrap();
    assert_eq!(second.assets_seen, 2);
    assert_eq!(second.assets_published_new, 0);
    assert_eq!(second.assets_already_published, 2);
    assert_eq!(second.blobs_written, 0);
    assert_eq!(second.blobs_deduped, 4);
    assert_eq!(core.catalog().resolve_asset_alias(&alias_a).unwrap(), Some(head_a));
    assert_eq!(core.catalog().resolve_asset_alias(&alias_b).unwrap(), Some(head_b));
}

#[test]
fn seed_with_non_derived_id_is_refused() {
    let (_root, core) = open_core("seed_bad_id");
    let seed = TestSeed { corrupt_first_id: true };
    let err = core.apply_stock_seed(&seed, NOW).unwrap_err();
    assert!(
        matches!(err, ServerError::Conflict { what: "seed asset_id not derived from alias" }),
        "{err}"
    );
}

/// Two entries claiming the same alias with different content: apply order
/// would silently pick a winner, so the whole seed must refuse up front.
struct DupSeed;

impl StockSeedSource for DupSeed {
    fn name(&self) -> &'static str {
        "dup-seed"
    }
    fn assets(&self) -> Vec<SeedAsset> {
        [b"first crate glb".to_vec(), b"second crate glb".to_vec()]
            .into_iter()
            .map(|glb| {
                let alias: AssetAlias = ALIAS_A.parse().unwrap();
                let id = stock_asset_id(&alias);
                let thumb = b"dup thumb".to_vec();
                SeedAsset {
                    manifest: prop_manifest(id, &glb, &thumb),
                    blobs: vec![glb, thumb],
                    alias,
                }
            })
            .collect()
    }
}

#[test]
fn duplicate_seed_aliases_refuse_before_any_mutation() {
    let (_root, core) = open_core("seed_dup");
    let err = core.apply_stock_seed(&DupSeed, NOW).unwrap_err();
    assert!(matches!(err, ServerError::Conflict { what: "duplicate seed alias" }), "{err}");
    // Deterministic refusal left no partial state: no alias head, no blobs.
    let alias: AssetAlias = ALIAS_A.parse().unwrap();
    assert_eq!(core.catalog().resolve_asset_alias(&alias).unwrap(), None);
    assert!(!core.cas().contains(&BlobId::hash_of(b"first crate glb")));
    assert!(!core.catalog().has_blob(&BlobId::hash_of(b"dup thumb")).unwrap());
}
