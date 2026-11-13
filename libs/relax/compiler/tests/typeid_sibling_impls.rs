// Two impls in one fn body, or in two `const _` blocks of one file (serde-derive's shape): their
// same-named methods have one def path, and so did the local types, closures and enum variants
// declared in them, so their TypeIds were equal and a downcast took one for the other. rustc's
// DefPath names each by its parent's, whose disambiguator tells the impls apart.
use std::any::{Any, TypeId};

fn type_of<T: 'static>(_: &T) -> TypeId {
    TypeId::of::<T>()
}

trait Probe {
    fn probe(&self) -> (TypeId, Box<dyn Any>, TypeId);
}

fn apart(what: &str, a: (TypeId, Box<dyn Any>, TypeId), b: (TypeId, Box<dyn Any>, TypeId)) {
    assert_ne!(a.0, b.0, "{what}: the local types of sibling methods");
    assert!((*a.1).type_id() == a.0 && (*b.1).type_id() != a.0, "{what}: a downcast across them");
    assert_ne!(a.2, b.2, "{what}: their closures");
}

struct C;
struct D;

const _: () = {
    impl Probe for C {
        fn probe(&self) -> (TypeId, Box<dyn Any>, TypeId) {
            struct Local;
            let c = || 1u8;
            (TypeId::of::<Local>(), Box::new(Local), type_of(&c))
        }
    }
};

const _: () = {
    impl Probe for D {
        fn probe(&self) -> (TypeId, Box<dyn Any>, TypeId) {
            struct Local;
            let c = || 2u8;
            (TypeId::of::<Local>(), Box::new(Local), type_of(&c))
        }
    }
};

#[test]
fn impls_in_two_const_blocks() {
    apart("const blocks", C.probe(), D.probe());
}

#[test]
fn impls_in_one_fn_body() {
    struct A;
    struct B;
    impl Probe for A {
        fn probe(&self) -> (TypeId, Box<dyn Any>, TypeId) {
            struct Local;
            let c = || 1u8;
            (TypeId::of::<Local>(), Box::new(Local), type_of(&c))
        }
    }
    impl Probe for B {
        fn probe(&self) -> (TypeId, Box<dyn Any>, TypeId) {
            struct Local;
            let c = || 2u8;
            (TypeId::of::<Local>(), Box::new(Local), type_of(&c))
        }
    }
    impl A {
        fn make() -> u8 {
            1
        }
    }
    impl B {
        fn make() -> u8 {
            2
        }
    }
    apart("one fn body", A.probe(), B.probe());
    assert_ne!(type_of(&A::make), type_of(&B::make), "inherent fns of sibling impls");
    assert_eq!(A::make() + B::make(), 3);
    let first = {
        enum E {
            V(u8),
        }
        let v = E::V;
        assert_eq!(match v(1) { E::V(x) => x }, 1);
        type_of(&v)
    };
    let second = {
        enum E {
            V(u8),
        }
        let v = E::V;
        assert_eq!(match v(2) { E::V(x) => x }, 2);
        type_of(&v)
    };
    assert_ne!(first, second, "the variant constructors of two enums of one name in one body");
}
