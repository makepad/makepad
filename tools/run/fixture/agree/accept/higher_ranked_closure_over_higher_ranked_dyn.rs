// A closure with a higher-ranked signature over a reference to a `dyn` whose own signature is higher-ranked
// (`Option<&dyn Fn(&B)>::filter(|_| ..)`): neither binder adds a bound on the outer lifetime.
pub type Bounds = ([f64; 3], [f64; 3]);
pub fn pred<'a>(slope: Option<&'a dyn Fn(&Bounds, f64) -> f64>, v: f64) -> bool { slope.filter(|_| v > 0.0).is_some() }
pub struct Prover<'a> { pub slope: Option<&'a dyn Fn(&Bounds, f64) -> f64>, pub lip: &'a dyn Fn() -> f64 }
impl Prover<'_> {
    pub fn radius(&self, c: [f64; 3], v: f64) -> f64 {
        let at = |c: [f64; 3], v: f64| self.slope.filter(|_| v > 0.0).map_or(f64::INFINITY, |slope| slope(&(c.map(|x| x - v), c.map(|x| x + v)), v));
        let local = at(c, v);
        v / if local.is_finite() { local.max(1.0) } else { (self.lip)().max(1.0) }
    }
}
#[test] fn runs() {
    let s = |b: &Bounds, v: f64| b.1[0] - b.0[0] + v;
    assert!(pred(Some(&s), 1.0) && !pred(Some(&s), -1.0) && !pred(None, 1.0));
    let p = Prover { slope: Some(&s), lip: &|| 2.0 };
    assert_eq!(p.radius([0.0; 3], 1.0), 1.0 / 3.0);
    assert_eq!(Prover { slope: None, lip: &|| 2.0 }.radius([0.0; 3], 1.0), 0.5);
}
