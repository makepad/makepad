//! The one generation cache: a recipe digest names what a run makes, and a
//! run on a fixed seed whose digest is already in the store returns the
//! stored result instead of submitting jobs. A drawn seed never hits.
//!
//! `digest = sha256(recipe id, recipe revision (its source), canonical
//! params, seed, ordered input content digests)`. Models are in the recipe
//! or its params. `Publish` records the digest with the result, and the
//! primary product of a fixed-seed run carries the alias [`reuse_alias`], so
//! a later run finds it in the app's own [`ResultStore`]
//! (`find_by_digest`).

use makepad_ai_hub::sha256::{to_hex, Sha256};
use makepad_flow::engine::results::{ResultInfo, ResultRef, ResultStore};
use makepad_strict_json::Value as Json;

/// What a run is made of, for its digest.
pub struct RecipeRun<'a> {
    /// The preset or flow the recipe is known by.
    pub recipe: &'a str,
    /// The recipe document as it runs.
    pub source: &'a str,
    /// Text inputs and parameter overrides, by name.
    pub params: &'a [(String, Json)],
    pub seed: u64,
    /// The content digests of the media inputs, in input order.
    pub inputs: &'a [String],
}

pub fn recipe_digest(run: &RecipeRun) -> [u8; 32] {
    let mut sha = Sha256::new();
    let mut field = |bytes: &[u8]| {
        sha.update(&(bytes.len() as u64).to_le_bytes());
        sha.update(bytes);
    };
    field(b"makepad-gen-recipe-v1");
    field(run.recipe.as_bytes());
    let mut source = Sha256::new();
    source.update(run.source.as_bytes());
    field(&source.finish());
    let mut params: Vec<(String, Json)> = run.params.to_vec();
    params.sort_by(|a, b| a.0.cmp(&b.0));
    field(Json::Obj(params).to_json().as_bytes());
    field(&run.seed.to_le_bytes());
    for input in run.inputs {
        field(input.as_bytes());
    }
    sha.finish()
}

pub fn hex(digest: &[u8; 32]) -> String {
    to_hex(digest)
}

/// The alias a fixed-seed run's primary product carries in `namespace`.
pub fn reuse_alias(namespace: &str, digest: &[u8; 32]) -> String {
    format!("{namespace}/gen-{}", &hex(digest)[..32])
}

/// A stored product made by this recipe digest, with what the store says
/// about it.
pub fn find(store: &dyn ResultStore, digest: &[u8; 32]) -> Option<(ResultRef, ResultInfo)> {
    let found = store.find_by_digest(&hex(digest))?;
    let info = store.info(&found)?;
    Some((found, info))
}
