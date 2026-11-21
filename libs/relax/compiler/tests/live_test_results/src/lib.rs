use std::sync::atomic::{AtomicU32, Ordering};

pub fn value() -> u32 { 1 }

/// What `fresh_statics` added in this run: a rerun after an edit starts it at 0, as a fresh run does.
static ADDED: AtomicU32 = AtomicU32::new(0);

#[test]
fn value_is_one() { assert_eq!(value(), 1); }

#[test]
fn fresh_statics() { assert_eq!(ADDED.fetch_add(value(), Ordering::SeqCst), 0); }

/// A data file an item reads (a fixture's text): its edit reaches the tests through this item.
const NOTE: &str = include_str!("note.txt");

#[test]
fn independent() { assert_eq!(2 + 2, 4); assert!(!NOTE.is_empty()); }

#[test]
#[ignore]
fn ignored() { panic!("ignored"); }

#[test]
fn filtered() { panic!("filtered"); }
