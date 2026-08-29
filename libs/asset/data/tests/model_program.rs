mod common;

use makepad_asset_data::{
    AssetDataError, AssetFile, AssetKind, DeviceTier, FileRole, MediaType,
};

fn model_program_manifest() -> makepad_asset_data::AssetManifest {
    let mut manifest = common::weapon_manifest();
    manifest.kind = AssetKind::ModelProgram;
    manifest.capabilities.spawnable = false;
    manifest.spawn_recipe = None;
    manifest.files.push(AssetFile {
        role: FileRole::Source,
        tier: DeviceTier::Any,
        lod: 0,
        media: MediaType::Text,
        blob: common::blob(8),
        byte_len: 40,
        dims: None,
    });
    manifest.metrics.total_bytes += 40;
    manifest
}

#[test]
fn model_program_requires_text_source_and_keeps_legacy_manifest_encoding_valid() {
    let manifest = model_program_manifest();
    let bytes = manifest.to_canonical_bytes().expect("valid model-program");
    let decoded = makepad_asset_data::AssetManifest::from_canonical_bytes(&bytes)
        .expect("model-program round trip");
    assert_eq!(decoded, manifest);

    let mut missing = manifest.clone();
    missing.files.retain(|file| file.role != FileRole::Source);
    missing.metrics.total_bytes -= 40;
    assert!(matches!(
        missing.validate(),
        Err(AssetDataError::Missing {
            what: "model-program source role"
        })
    ));

    let mut wrong_media = manifest;
    wrong_media
        .files
        .iter_mut()
        .find(|file| file.role == FileRole::Source)
        .unwrap()
        .media = MediaType::Bin;
    assert!(matches!(
        wrong_media.validate(),
        Err(AssetDataError::Missing {
            what: "model-program source role"
        })
    ));
}
