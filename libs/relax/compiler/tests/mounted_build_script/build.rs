// A build script of a workspace mounted from a git store (a devhub overlay, `run --src`): its run
// inherits the mount table and the mount's std, a path on no disk, and must compile and run.
fn main() {
    println!("cargo:rustc-env=MOUNTED_SCRIPT=ran");
}
