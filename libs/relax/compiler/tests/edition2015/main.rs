// `makepad run --units tests/edition2015/units.json.in (after @R@ substitution) app`: an edition-2015
// library's crate-root-relative `use` paths (boot B17). rustc: exits 0.
fn main() {
    assert!(e2015::f() == 4);
    assert!(e2015::prelude_paths::sizes() == 15);
}
