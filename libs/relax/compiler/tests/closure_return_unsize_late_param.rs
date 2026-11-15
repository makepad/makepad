//! A closure `|s| s` coerced to `fn(&mut S) -> &mut dyn Named` while `S` is still unknown: its
//! return is an unsizing coercion that waits for `S` (rustc's "ambiguous unsize case" of a sized
//! type variable), not `S = dyn Named`.
pub trait Named {
    fn name(&self) -> u32;
}

pub struct Thing(pub u32);

impl Named for Thing {
    fn name(&self) -> u32 {
        self.0
    }
}

pub struct Kind<S> {
    pub get: fn(&mut S) -> &mut dyn Named,
}

pub enum AppKind<S> {
    Getter(Box<Kind<S>>),
    Plain,
}

pub struct Harness<S> {
    kind: AppKind<S>,
    state: S,
}

impl<S> Harness<S> {
    pub fn new(kind: AppKind<S>, state: S) -> Self {
        Harness { kind, state }
    }
    pub fn name(&mut self) -> u32 {
        match &self.kind {
            AppKind::Getter(k) => (k.get)(&mut self.state).name(),
            AppKind::Plain => 0,
        }
    }
}

#[test]
fn a_struct_literals_closure_before_the_parameter_type() {
    let kind = Kind { get: |s| s };
    let mut t = Thing(7);
    assert_eq!((kind.get)(&mut t).name(), 7);
}

#[test]
fn a_boxed_closure_in_a_variant_whose_type_a_later_argument_fixes() {
    let mut h = Harness::new(AppKind::Getter(Box::new(Kind { get: |s| s })), Thing(9));
    assert_eq!(h.name(), 9);
}
