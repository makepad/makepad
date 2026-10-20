//! Relax's base: what every phase shares and nothing above it -- hashing (fxhash, sip), memory accounting (memstat,
//! zone_alloc), clocks, concurrent containers (conc, arena), the lexer and syntax tree (lexer, ast), cfg, the
//! manifests (toml, resolve), the git read path and source mounts (git, gitstore, vfs, inflate)
//! and the proc-macro token model (pm, pm_tokens).
//! The layers below are re-exported, so every module of Relax is `crate::<module>` in each layer.

#![recursion_limit = "1024"]


pub mod fxhash;
pub mod memstat;
pub use relax_session::{gen_local, options, sip, Session};
pub mod conc;
pub mod pool;
pub mod query;
pub mod preempt;
pub mod stack;
pub mod arena;
pub mod json;
pub mod diag;
pub use relax_lex::lexer;
pub use relax_lex::packed;
pub mod ast;
pub mod toml;
pub mod inflate;
pub mod git;
pub mod gitstore;
pub mod vfs;
pub mod packsrc;
pub mod progress;
pub mod run_state;
pub mod boot_stats;
pub mod dirs;
pub use relax_lex::tokens as pm_tokens;
pub mod pm;
pub mod zone_alloc;
pub mod clock;
pub mod relaxed;
pub mod srchash;
pub mod simd;
pub mod thread_local_mac;
pub mod tokbuf;
pub mod cfg;
pub mod registry;
pub mod resolve;
pub mod sha256;
pub mod std_digest;
pub mod tar;
pub mod image;
pub mod why;
pub mod waits;

pub mod scc;

pub mod solver_work;
