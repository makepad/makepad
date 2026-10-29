// a build script using a library that links proc_macro (web_atoms' build script uses
// string_cache_codegen -> proc-macro2 this way)
fn main() {
    println!("cargo:rustc-env=PM_BUILD={} {}", pmlib::avail(), pmlib::tokens("a + b"));
}
