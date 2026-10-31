// The app graph sees proc-macro crates (and the crates only they use) as this empty crate:
// they run in the JIT proc-macro host (src/pmhost.rs), not in the app.
