// Higher-ranked fn pointers implement Debug and Pointer (real core:
// library/core/src/ptr/mod.rs `impl<F: FnPtr> fmt::Debug for F`), so a
// `&for<'a> fn(&'a mut T)` unsize-coerces to `&dyn Debug` as #[derive(Debug)]
// needs for fields holding such pointers or Options/tuples/slices of them.
use std::any::Any;
use std::fmt::{self, Debug};
use std::sync::Arc;

struct Registry {
    count: u32,
}

type Register = fn(&mut Registry, Option<u32>) -> Result<u32, String>;
type Leaf = fn(&mut Registry) -> Result<u32, String>;

#[derive(Clone, Copy, Debug)]
enum FieldType {
    Builtin(&'static str),
    Registered(Register),
}

#[derive(Clone, Default, Debug)]
struct Vars {
    lookup: Option<fn(&dyn Any) -> Option<u32>>,
    write: Option<fn(&mut dyn Any, &dyn Any) -> bool>,
    leaf: Option<Leaf>,
    pair: Option<(Leaf, u8)>,
    chain: Option<Arc<[Register]>>,
    styles: Option<fn(&mut Registry)>,
}

fn register(registry: &mut Registry, extra: Option<u32>) -> Result<u32, String> {
    registry.count += extra.unwrap_or(1);
    Ok(registry.count)
}

fn leaf(registry: &mut Registry) -> Result<u32, String> {
    Ok(registry.count)
}

fn lookup(_value: &dyn Any) -> Option<u32> {
    None
}

fn bump(registry: &mut Registry) {
    registry.count += 1;
}

fn shown<T: Debug + ?Sized>(value: &T) -> String {
    format!("{:?}", value)
}

fn pointed<T: fmt::Pointer>(value: T) -> String {
    format!("{:p}", value)
}

#[test]
fn higher_ranked_fn_pointers_are_debug() {
    let address = format!("{:p}", register as Register as *const ());
    let registered = FieldType::Registered(register);
    assert_eq!(format!("{:?}", registered), format!("Registered({})", address));
    assert_eq!(format!("{:?}", FieldType::Builtin("f32")), "Builtin(\"f32\")");

    let pointer: Register = register;
    assert_eq!(format!("{:?}", pointer), address);
    assert_eq!(format!("{:p}", pointer), address);
    let dynamic: &dyn Debug = &pointer;
    assert_eq!(format!("{:?}", dynamic), address);
    assert_eq!(pointed(pointer), address);
    assert_eq!(shown(&pointer), address);

    let explicit: for<'a> fn(&'a mut Registry) -> Result<u32, String> = leaf;
    let leaf_address = format!("{:p}", explicit as *const ());
    assert_eq!(shown(&explicit), leaf_address);

    let vars = Vars {
        lookup: Some(lookup),
        leaf: Some(leaf),
        pair: Some((leaf, 7)),
        chain: Some(Arc::from(vec![pointer, pointer])),
        styles: Some(bump),
        ..Vars::default()
    };
    let text = format!("{:?}", vars);
    assert!(text.starts_with("Vars { lookup: Some(0x"), "{}", text);
    assert!(text.contains("write: None"), "{}", text);
    assert!(text.contains(&format!("leaf: Some({})", leaf_address)), "{}", text);
    assert!(text.contains(&format!("pair: Some(({}, 7))", leaf_address)), "{}", text);
    assert!(text.contains(&format!("chain: Some([{}, {}])", address, address)), "{}", text);

    assert!(vars.lookup.is_some_and(|f| f(&1u8).is_none()));
    assert!(vars.write.is_none());
    assert_eq!(vars.pair.map(|(_, n)| n), Some(7));
    assert_eq!(vars.chain.as_ref().map(|c| c.len()), Some(2));
    if let FieldType::Registered(f) = registered {
        assert!(std::ptr::fn_addr_eq(f, pointer));
    }
    if let FieldType::Builtin(name) = FieldType::Builtin("f32") {
        assert_eq!(name, "f32");
    }

    let mut registry = Registry { count: 1 };
    assert_eq!(pointer(&mut registry, Some(2)), Ok(3));
    (vars.styles.unwrap())(&mut registry);
    assert_eq!((vars.leaf.unwrap())(&mut registry), Ok(4));
    assert!(std::ptr::fn_addr_eq(pointer, register as Register));
}
