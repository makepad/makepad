//! Build-time inputs shared by native macro matching and source identity checks.
use std::path::Path;

/// Watch directories as well as files so additions and removals change the identity.
/// Include manifests, build scripts and every source asset, using checkout-relative names.
pub fn package(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) {
        println!("cargo:rerun-if-changed={}", dir.display());
        for entry in std::fs::read_dir(dir).expect("read source directory") {
            let entry = entry.expect("source entry");
            let path = entry.path();
            if entry.file_type().expect("source file type").is_dir() {
                collect(root, &path, files);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_str().expect("UTF-8 source path").replace('\\', "/");
                files.push((relative, std::fs::read(&path).expect("read source file")));
            }
        }
    }
    let mut files = Vec::new();
    println!("cargo:rerun-if-changed={}", root.display());
    for name in ["Cargo.toml", "build.rs"] {
        let path = root.join(name);
        if path.is_file() {
            files.push((name.to_string(), std::fs::read(path).expect("read package input")));
        }
    }
    collect(root, &root.join("src"), &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}
