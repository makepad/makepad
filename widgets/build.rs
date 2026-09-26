use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;

// Records where the widgets directory (its resources/ and themes/) is, next
// to the build's artifacts: target/<profile>/makepad-widgets.path. This is the
// front crate's manifest directory, the same for every family mix, and it
// only changes when the checkout moves, so the script reruns on its own edit
// only (a build script without a rerun directive reruns on any change in the
// package).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = env::var("OUT_DIR").unwrap();
    let path = Path::new(&out_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let cwd = std::env::current_dir().unwrap();
    let mut file = File::create(path.join("makepad-widgets.path")).unwrap();
    file.write_all(&format!("{}", cwd.display()).as_bytes())
        .unwrap();
}
