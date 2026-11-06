// A derive copies a field's type tokens into the code it generates (`AssertParamIsClone<..>`).
// The parser splits compound tokens in place while it reads generic arguments (`<-25` is `<-`
// then `25`; `<<T as Tr>::A>` is `<<`), so the copy must give the first half back. The `time`
// crate's `Parsed` struct (`Option_ri8<-25, 25>` under `#[derive(Clone, Copy)]`) is the shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ranged<const MIN: i32, const MAX: i32>(i32);
pub type OptRanged<const MIN: i32, const MAX: i32> = Option<Ranged<MIN, MAX>>;

pub trait Tr { type A; }
impl Tr for u8 { type A = u16; }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parsed {
    offset_hour: OptRanged<-25, 25>,
    offset_minute: OptRanged<{ -Ranged::<0, 1>::per() + 1 }, { Ranged::<0, 1>::per() - 1 }>,
    assoc: Option<<u8 as Tr>::A>,
    shifted: [u8; { 1 << 2 }],
}

impl<const MIN: i32, const MAX: i32> Ranged<MIN, MAX> {
    const fn per() -> i32 { 60 }
}

#[test]
fn a01_clone_copy_struct_with_negative_const_args() {
    let p = Parsed { offset_hour: Some(Ranged(-3)), offset_minute: None, assoc: Some(7), shifted: [1, 2, 3, 4] };
    let q = p.clone();
    println!("| {:?} {} {}", q.offset_hour, q.offset_minute.is_none(), q == p);
    println!("| {:?} {:?}", q.assoc, q.shifted);
}

#[test]
fn a02_clone_copy_enum_with_split_tokens() {
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum E { A(OptRanged<-1, 1>), B(<u8 as Tr>::A) }
    let e = E::A(Some(Ranged(1)));
    println!("| {:?} {:?} {}", e.clone(), E::B(3).clone(), e == E::A(Some(Ranged(1))));
}
