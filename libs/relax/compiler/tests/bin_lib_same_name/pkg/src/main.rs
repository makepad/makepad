// The package's bin has the lib's crate name (cargo names both `binlib_pkg`) and reaches the
// dependency's exported macro through the lib: `binlib_pkg::` is the lib here, not the bin.
use binlib_pkg::binlib_dep::*;
use binlib_pkg::answer;

entry!(answer);
