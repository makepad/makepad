// relax issue 43: an expression path through a generic associated type that leaves out its
// lifetime (`<F as Form>::Mul::from(params)` for `type Mul<'a>`) infers it, as rustc does. Relax
// lowered it as an error region, which failed the body with only "type checking failed due to
// previous errors".
pub trait Form: Sized {
    type Params: 'static;
    type Mul<'a>: Mult<'a, Form = Self>;
    fn value(&self) -> u64;
}

pub trait Mult<'a>: From<&'a <Self::Form as Form>::Params> {
    type Form: Form;
    fn mul_assign(&mut self, lhs: &mut Self::Form, rhs: &Self::Form);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Res(u64);

pub struct ResMul<'a>(&'a u64);

impl<'a> From<&'a u64> for ResMul<'a> {
    fn from(m: &'a u64) -> Self {
        ResMul(m)
    }
}

impl<'a> Mult<'a> for ResMul<'a> {
    type Form = Res;
    fn mul_assign(&mut self, lhs: &mut Res, rhs: &Res) {
        lhs.0 = lhs.0 * rhs.0 % *self.0;
    }
}

impl Form for Res {
    type Params = u64;
    type Mul<'a> = ResMul<'a>;
    fn value(&self) -> u64 {
        self.0
    }
}

fn square_concrete(params: &u64, x: Res) -> Res {
    let mut mm = <Res as Form>::Mul::from(params);
    let mut y = x;
    mm.mul_assign(&mut y, &x);
    y
}

fn square_generic<F: Form + Copy>(params: &F::Params, x: F) -> F {
    let mut mm = <F as Form>::Mul::from(params);
    let mut y = x;
    mm.mul_assign(&mut y, &x);
    y
}

#[test]
fn an_elided_gat_lifetime_in_a_value_path_is_inferred() {
    assert_eq!(square_concrete(&7, Res(3)).value(), 2, "the concrete form");
    assert_eq!(square_generic(&7, Res(4)).value(), 2, "the generic form");
}
