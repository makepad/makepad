#[derive(Default)]
struct Metadata { name: Option<String>, updated_at: Option<f64> }
fn metadata(value: &Metadata) -> impl PartialEq + '_ { (&value.name, value.updated_at) }
fn opaque<T: PartialEq>(value: T) -> impl PartialEq { value }

#[test]
fn a01_indirect_opaque_tuples_compare_their_values() {
    let first = Metadata { name: Some("Title".into()), ..Default::default() };
    let renamed = Metadata { name: Some("Renamed".into()), ..Default::default() };
    let same = Metadata { name: Some("Title".into()), ..Default::default() };
    assert!(metadata(&first) != metadata(&renamed));
    assert!(metadata(&first) == metadata(&same));
    println!("| indirect opaque tuple uses value equality");
}

#[test]
fn a02_large_float_arrays_preserve_inequality_and_nan() {
    let zero = opaque([0.0f64; 128]);
    let one = opaque([1.0f64; 128]);
    assert!(zero != one);
    assert!(zero == opaque([0.0f64; 128]));
    let nan = opaque([f64::NAN; 128]);
    assert!(nan != nan);
    println!("| indirect arrays preserve floating-point equality");
}

struct Key { value: u8, incidental: u8 }
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool { self.value == other.value }
}
#[test]
fn a03_opaque_values_respect_custom_equality() {
    let a = Key { value: 1, incidental: 2 };
    let b = Key { value: 1, incidental: 3 };
    assert_ne!(a.incidental, b.incidental);
    assert!(opaque(a) == opaque(b));
    println!("| assertions respect the type's PartialEq implementation");
}

#[test]
fn a04_nested_references_compare_the_referenced_values() {
    assert!(opaque(&"a") != opaque(&"b"));
    assert!(opaque(&"a") == opaque(&"a"));
    let first = String::from("value");
    let same = String::from("value");
    let different = String::from("changed");
    let first_ref = &first;
    let same_ref = &same;
    let different_ref = &different;
    assert!(opaque(&first_ref) == opaque(&same_ref));
    assert!(opaque(&first_ref) != opaque(&different_ref));
    println!("| nested references preserve value equality");
}
