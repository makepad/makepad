//! Where Synth keeps its files: wherever `SYNTH_DATA_DIR` says (a test run
//! keeps its data apart), else `local/synth` in the Makepad checkout Synth
//! runs in, else (an installed Synth) the app's data folder. Inside it:
//! `scores/` (the library, `*.score`), `drafts/` (unsaved changes),
//! `samples/` (audio a score plays by name), `renders/` (WAV exports),
//! `resources/` (the downloads: the GM SoundFont, the MIDI datasets) and
//! `batches/` (songs written at once).

use std::path::PathBuf;

/// The data folder, resolved once.
pub fn data_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(dir) = std::env::var_os("SYNTH_DATA_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
            return dir;
        }
        // Not on Android: there the checkout is the APK's read-only source pack.
        #[cfg(not(target_os = "android"))]
        if let Some(dir) = checkout_data_dir(std::env::current_dir().ok().as_deref(), std::env::current_exe().ok().as_deref()) {
            return dir;
        }
        makepad_widgets::makepad_platform::home::app_data_dir("synth").unwrap_or_else(|| PathBuf::from("synth-data"))
    }).clone()
}

/// `local/synth` of the Makepad checkout Synth runs in: the working
/// directory's, else (a binary started from outside one) the binary's. Not
/// the binary's first: under `makepad run` it is the box's shared JIT
/// loader, and every Synth it runs (CI legs, lanes, the dock's) would share
/// its drafts and reopen whatever one of them left open.
#[cfg(not(target_os = "android"))]
fn checkout_data_dir(cwd: Option<&std::path::Path>, exe: Option<&std::path::Path>) -> Option<PathBuf> {
    [cwd, exe].into_iter().flatten().find_map(|start| {
        start.ancestors().find(|root| root.join("makepad.splash").is_file() && root.join("apps/synth/Cargo.toml").is_file()).map(|root| root.join("local/synth"))
    })
}

/// A folder of the data folder, made on first use.
fn sub(name: &str) -> PathBuf {
    let dir = data_dir().join(name);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn scores_dir() -> PathBuf {
    sub("scores")
}

pub fn drafts_dir() -> PathBuf {
    sub("drafts")
}

pub fn samples_dir() -> PathBuf {
    sub("samples")
}

pub fn renders_dir() -> PathBuf {
    sub("renders")
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use std::path::Path;

    fn checkout(root: &Path) {
        std::fs::create_dir_all(root.join("apps/synth")).unwrap();
        std::fs::write(root.join("makepad.splash"), "").unwrap();
        std::fs::write(root.join("apps/synth/Cargo.toml"), "").unwrap();
    }

    /// A CI leg or lane run starts Synth in its own tree through the box's
    /// shared JIT loader: the tree decides, so it never reopens a draft
    /// another Synth left in the loader's checkout (CI run 130 booted a
    /// stale edited draft there and logged 9 script errors).
    #[test]
    fn the_working_checkout_decides_before_the_binarys() {
        let dir = std::env::temp_dir().join(format!("synth-test-{}-checkouts", std::process::id()));
        let (tree, loader) = (dir.join("tree"), dir.join("box"));
        checkout(&tree);
        checkout(&loader);
        let exe = loader.join("target/host/makepad");
        let in_tree = super::checkout_data_dir(Some(&tree.join("apps/synth")), Some(&exe));
        let outside = super::checkout_data_dir(Some(&dir), Some(&exe));
        let neither = super::checkout_data_dir(Some(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(in_tree, Some(tree.join("local/synth")), "started in a checkout: its data");
        assert_eq!(outside, Some(loader.join("local/synth")), "started outside one: the binary's checkout");
        assert_eq!(neither, None, "neither in a checkout");
    }
}
