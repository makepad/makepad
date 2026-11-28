#[derive(PartialEq)]
pub enum Role {
    Host,
    Viewer { sharer: u32 },
    EmptyStruct {},
    Member(u32),
    EmptyTup(),
}

type Alias = Role;
struct Holder;
trait HasRole { type Out; }
impl HasRole for Holder { type Out = Role; }

macro_rules! ok { () => { Role::Host }; }
macro_rules! pass { ($p:path) => { $p }; }

impl Role {
    const DEFAULT: Self = Self::EmptyStruct {};

    fn is_host(&self) -> bool { matches!(self, Self::Host) }
    fn is_viewer(&self) -> bool { matches!(self, Self::Viewer { .. }) }
}

#[test]
fn unit_variant_paths() {
    use Role::Host;
    for r in [Role::Host, Role::Member(1)] {
        let host = r == Role::Host;
        assert_eq!(matches!(r, Role::Host), host);
        assert_eq!(r.is_host(), host);
        assert_eq!(matches!(r, Alias::Host), host);
        assert_eq!(matches!(r, <Role>::Host), host);
        assert_eq!(matches!(r, <Holder as HasRole>::Out::Host), host);
        assert_eq!(matches!(r, Host), host);
        assert_eq!(matches!(r, ok!()), host);
        assert_eq!(matches!(r, pass!(Role::Host)), host);
    }
}

struct Unit;
impl Unit {
    fn by_self(self) -> u32 { match self { Self => 3 } }
}

#[test]
fn unit_struct_paths() {
    let a = match Unit { Unit => 1 };
    let b = match Unit { self::Unit => 2 };
    assert_eq!((a, b, Unit.by_self()), (1, 2, 3));
}

const FLAG: u32 = 4;
const REF_UNIT: &() = &();
trait DefaultRole { const DEFAULT: Role; }
impl DefaultRole for Role { const DEFAULT: Role = Role::EmptyTup(); }
pub trait Both { type X; const X: u32; }
impl Both for Role {
    const X: u32 = 19;
    type X = u32;
}

fn is_flag<const N: u32>() -> bool { matches!(N, FLAG) }

#[test]
fn constant_paths() {
    assert!(is_flag::<4>());
    assert!(!is_flag::<5>());
    let through_ref = match &() { REF_UNIT => 1 };
    assert_eq!(through_ref, 1);
    assert!(matches!(19u32, <Role as Both>::X));
    assert!(!matches!(4u32, <Role as Both>::X));
    assert!(matches!(Role::EmptyTup(), <Role as DefaultRole>::DEFAULT));
    assert!(!matches!(Role::Host, <Role as DefaultRole>::DEFAULT));
    for r in [Role::Host, Role::EmptyStruct {}, Role::Member(2)] {
        assert_eq!(matches!(r, Role::Host | Role::DEFAULT), !matches!(r, Role::Member(_)));
    }
}

struct Pair(u32);
impl Pair {
    fn field(self) -> u32 { let Self(x) = self; x }
    fn assign() -> u32 {
        let c;
        Self(c) = Pair(9);
        c
    }
}

#[test]
fn braced_and_tuple_patterns() {
    use Role::Viewer;
    let r = Viewer { sharer: 7 };
    assert!(r.is_viewer());
    assert!(matches!(r, Role::Viewer { .. }));
    assert!(matches!(r, Alias::Viewer { .. }));
    assert!(matches!(r, Viewer { .. }));
    let field = match r { Role::Viewer { sharer } => sharer, _ => 0 };
    assert_eq!(field, 7);
    assert!(matches!(Role::EmptyStruct {}, Role::EmptyStruct {}));
    assert!(matches!(Role::Member(8), Role::Member(_)));
    assert!(matches!(Role::EmptyTup(), Role::EmptyTup()));
    let p = Pair(6);
    let Pair(..) = p;
    let Pair { 0: x } = p;
    assert_eq!(x, 6);
    assert_eq!(p.field(), 6);
}

enum Single { Only(u32) }
type AliasSingle = Single;

#[test]
fn tuple_constructor_assignments() {
    let (a, b, d, e, f);
    Pair(a) = Pair(7);
    self::Pair(b) = Pair(8);
    Single::Only(d) = Single::Only(10);
    AliasSingle::Only(e) = Single::Only(11);
    <Single>::Only(f) = Single::Only(12);
    assert_eq!((a, b, Pair::assign(), d, e, f), (7, 8, 9, 10, 11, 12));
}
