use std::sync::Arc;
pub struct GBuf { v: Vec<f32> }
impl GBuf { pub fn host(&self) -> &[f32] { &self.v } }
pub struct Grads { pub acc: Vec<f32> }
impl Grads { pub fn add_vec(&mut self, _id: usize, d: Vec<f32>) { for (a, b) in self.acc.iter_mut().zip(d) { *a += b; } } }
type Back = Box<dyn Fn(&GBuf, &mut Grads)>;
fn make(x: usize, scale: f32) -> Back {
    let rot = Arc::new(move |data: &[f32], sign: f32| data.iter().map(|v| v * sign * scale).collect::<Vec<f32>>());
    let rb = rot.clone();
    let back: Back = Box::new(move |d, g| g.add_vec(x, rb(d.host(), -1.0)));
    back
}
#[test]
fn closure_params_from_box_dyn_expectation() {
    let b = make(0, 2.0);
    let mut g = Grads { acc: vec![0.0, 0.0] };
    b(&GBuf { v: vec![1.0, 2.0] }, &mut g);
    assert_eq!(g.acc, vec![-2.0, -4.0]);
}

// Exercise capture analysis of Deref + Fn independently of Box's expectation.
#[test]
fn captured_arc_callable_with_explicit_parameters() {
    let scale = 3.0f32;
    let rot = Arc::new(move |value: f32| value * scale);
    let rb = rot.clone();
    let back = move |value: f32| rb(value);
    assert_eq!(back(2.0), 6.0);
    assert_eq!(back(-1.0), -3.0);
}

// Exercise signature deduction independently of the captured callable's deref.
#[test]
fn box_dyn_expectation_without_captured_callable() {
    let back: Back = Box::new(move |d, g| g.add_vec(0, d.host().to_vec()));
    let mut g = Grads { acc: vec![1.0, 2.0] };
    back(&GBuf { v: vec![3.0, 4.0] }, &mut g);
    assert_eq!(g.acc, vec![4.0, 6.0]);
}
