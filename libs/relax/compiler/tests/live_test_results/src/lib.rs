pub fn value() -> u32 { 1 }

#[test]
fn value_is_one() { assert_eq!(value(), 1); }

#[test]
fn independent() { assert_eq!(2 + 2, 4); }

#[test]
#[ignore]
fn ignored() { panic!("ignored"); }

#[test]
fn filtered() { panic!("filtered"); }
