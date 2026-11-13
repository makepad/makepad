// The script's build-dependency is a native package of its own; the program's dependency is
// another, whose C calls back into its own Rust. The run links both into one native library and
// the script's run loads it: that C's callback is no fn of the script's program, which never calls
// that C (relax issue 174).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=SCRIPT_SAW={}", snd_plain::twice(21));
}
