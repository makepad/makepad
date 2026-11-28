//! Every crate the loader provides, as its build links them: what the loader's provided-statics
//! check loads (`makepad check --boundary statics relax-provided-all`, loader_build.sh). The check
//! fails for a provided crate the loader registers that this package does not depend on.
