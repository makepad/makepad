use super::*;

/// A tiny valid atlas + its glb stand-in on disk, with the sidecar
/// stamped strictly NEWER than the glb (the fresh case the gates are
/// then tightened around one axis at a time).
fn fixture(
    dir: &std::path::Path,
    hash: u64,
    sun: &SunLight,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let glb = dir.join("caster.glb");
    let sidecar = dir.join("caster.glb.shadowsdf");
    std::fs::write(&glb, b"not really a glb - only its mtime matters").unwrap();
    let atlas = crate::shadow_sdf::ShadowSdfAtlas {
        pixels: vec![
            200u8;
            crate::shadow_sdf::SDF_YAWS
                * crate::shadow_sdf::SDF_CELL
                * crate::shadow_sdf::SDF_CELL
        ],
        rows: 1,
        rect: (-1.0, -1.0, 2.0, 2.0),
        band_world: 0.25,
        len_per_unit: sun.shadow_len_per_unit(),
    };
    std::fs::write(&sidecar, atlas.to_shadowsdf(hash)).unwrap();
    // mtimes: glb one minute in the past, sidecar now — unambiguous
    // even on filesystems with coarse timestamps.
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(&glb)
        .unwrap()
        .set_modified(past)
        .unwrap();
    (glb, sidecar)
}

/// Every gate in `Renderer::load_shadow_sdf_sidecar`: fresh +
/// keyed + same-sun loads; a stale mtime, a foreign hash, or a
/// different sun each falls back to `None` (= the off-thread bake).
#[test]
fn sidecar_gates_hold() {
    let dir = std::env::temp_dir().join(format!("shadowsdf_gates_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sun = SunLight { dir: vec3f(0.55, 0.62, 0.56).normalize(), ..SunLight::default() };
    let (glb, sidecar) = fixture(&dir, 77, &sun);

    // Fresh, keyed, same sun: loads.
    let atlas = Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, Some(77), &sun)
        .expect("fresh keyed sidecar should load");
    assert_eq!(atlas.rows, 1);
    // No expected hash (the model case): also loads.
    assert!(Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, None, &sun).is_some());
    // Wrong hash: rejected.
    assert!(Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, Some(78), &sun).is_none());
    // A MILDLY different sun (an authored time_of_day): loads — the
    // instance build stretches the window by the length ratio
    // (play-session-1 entry 18; exact matching starved the whole tier).
    let mild =
        SunLight { dir: vec3f(0.45, 0.70, 0.46).normalize(), ..SunLight::default() };
    assert!(
        Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, Some(77), &mild).is_some()
    );
    // A WILDLY different sun (near-noon vs the low bake): outside the
    // 0.2-5x stretch band — rejected; a smeared stretch is worse than
    // the blob.
    let other = SunLight { dir: vec3f(0.1, 0.95, 0.1).normalize(), ..SunLight::default() };
    assert!(
        Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, Some(77), &other).is_none()
    );
    // Stale mtime rejects only an UNKEYED load (checkout files). A
    // keyed load trusts the hash: the store cache writes both files at
    // arbitrary times, and write ordering must not cost the shadows.
    let older = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
    std::fs::File::options()
        .write(true)
        .open(&sidecar)
        .unwrap()
        .set_modified(older)
        .unwrap();
    assert!(Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, None, &sun).is_none());
    assert!(Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, Some(77), &sun).is_some());
    // Missing either file: rejected, never a panic.
    assert!(Renderer::load_shadow_sdf_sidecar(
        &dir.join("absent.shadowsdf"),
        &glb,
        None,
        &sun
    )
    .is_none());
    assert!(Renderer::load_shadow_sdf_sidecar(
        &sidecar,
        &dir.join("absent.glb"),
        None,
        &sun
    )
    .is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
