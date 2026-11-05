//! The workspace's host build flags (`.cargo/config.toml`): rustc's parallel frontend under
//! `RUSTC_BOOTSTRAP=1`, with `-Z allow-features=` so that the bootstrap lets no `#![feature]`
//! through. A probe crate outside the tree, built by cargo from the workspace root (cargo reads
//! its config from the folder it runs in), shows both.

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::{Command, Output};

    /// `cargo build -v` of the probe crate from the workspace root, with nothing from the
    /// caller's environment standing in for the config's flags.
    fn build(root: &Path, probe: &Path, features: &[&str]) -> Output {
        let mut cmd = Command::new("cargo");
        cmd.current_dir(root).args(["build", "-v", "--offline", "--manifest-path"]).arg(probe.join("Cargo.toml")).arg("--target-dir").arg(probe.join("target")).args(features);
        for k in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "RUSTC_BOOTSTRAP", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET"] {
            cmd.env_remove(k);
        }
        cmd.output().unwrap()
    }

    // x86_64-pc-windows-gnu (Runner's Windows build) has no host table in the config
    #[cfg(not(all(windows, target_env = "gnu")))]
    #[test]
    fn host_builds_get_the_parallel_frontend_and_a_feature_attribute_fails_to_build() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(root.join(".cargo/config.toml").is_file(), "no workspace config at {}", root.display());
        let probe = std::env::temp_dir().join(format!("makepad-build-flags-{}", std::process::id()));
        std::fs::create_dir_all(probe.join("src")).unwrap();
        std::fs::write(probe.join("Cargo.toml"), "[package]\nname = \"build-flags-probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[features]\ngate = []\n\n[workspace]\n").unwrap();
        std::fs::write(probe.join("src/lib.rs"), "#![cfg_attr(feature = \"gate\", feature(test))]\npub fn len() -> usize { format!(\"{}\", 10).len() }\n").unwrap();

        let plain = build(&root, &probe, &[]);
        let log = String::from_utf8_lossy(&plain.stderr);
        assert!(plain.status.success(), "the probe without a feature attribute must build:\n{log}");
        assert!(log.contains("-Z threads=8") && log.contains("-Z allow-features="), "rustc ran without the config's flags:\n{log}");

        let gated = build(&root, &probe, &["--features", "gate"]);
        let log = String::from_utf8_lossy(&gated.stderr);
        assert!(!gated.status.success() && log.contains("E0725"), "a #![feature] must fail to build (E0725):\n{log}");
        let _ = std::fs::remove_dir_all(&probe);
    }
}
