//! devhub's client side: the names, paths and records every devhub role agrees on ([`proto`]), and
//! one ask of a devhub service ([`ask`]). A crate that only talks to devhub depends on this, not on
//! devhub itself, whose every change would otherwise be its change too.

pub mod ask;
pub mod proto;
