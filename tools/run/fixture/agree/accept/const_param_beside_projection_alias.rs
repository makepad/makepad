pub trait HasRole {
    type Out;
}

pub struct Holder;
pub enum Role {
    Viewer { sharer: u32 },
}

impl HasRole for Holder {
    type Out = Role;
}

type A = <Holder as HasRole>::Out;

pub fn g<const A: usize>() -> u32 {
    let r = A::Viewer { sharer: 1 };
    match r {
        A::Viewer { sharer } => sharer,
    }
}

#[test]
fn projection_alias_constructs_and_matches_the_variant() {
    assert_eq!(g::<0>(), 1);
}
