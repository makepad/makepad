// Capture analysis of a closure that calls another closure through an
// `Arc` (an overloaded deref whose target still holds the inner closure's
// inference variables) runs in the body's own inference context (rustc
// expr_use_visitor.rs: FnCtxt resolve_vars_if_possible /
// structurally_resolve_type), never a fresh one (makepad ai sing nn::rope).
use std::sync::Arc;

pub struct Buf(Vec<f32>);

impl Buf {
    pub fn host(&self) -> &[f32] {
        &self.0
    }
}

#[derive(Default)]
pub struct Grads {
    pub acc: Vec<f32>,
}

impl Grads {
    pub fn add_vec(&mut self, id: usize, d: Vec<f32>) {
        self.acc.resize(self.acc.len().max(id + d.len()), 0.0);
        for (i, v) in d.into_iter().enumerate() {
            self.acc[id + i] += v;
        }
    }
}

type Back = Box<dyn Fn(&Buf, &mut Grads)>;

pub struct Graph {
    vals: Vec<Arc<Vec<f32>>>,
    backs: Vec<Option<Back>>,
}

impl Graph {
    fn push(&mut self, t: Vec<f32>, back: Option<Back>) -> usize {
        self.vals.push(Arc::new(t));
        self.backs.push(back);
        self.vals.len() - 1
    }

    pub fn rope(&mut self, x: usize, heads: usize) -> usize {
        let xv = self.vals[x].clone();
        let c = xv.len();
        let half = c / heads / 2;
        let freqs: Vec<f32> = (0..half).map(|i| 1.0 + i as f32).collect();
        let rot = Arc::new(move |data: &[f32], sign: f32| {
            let mut y = data.to_vec();
            for h in 0..heads {
                for i in 0..half {
                    let a = h * half * 2 + i;
                    y[a] = data[a] * freqs[i] * sign;
                }
            }
            y
        });
        let y = rot(&xv, 1.0);
        let rb = rot.clone();
        let back: Back = Box::new(move |d, g| g.add_vec(x, rb(d.host(), -1.0)));
        self.push(y, Some(back))
    }
}

#[test]
fn capture_through_overloaded_deref_of_closure() {
    let mut g = Graph { vals: vec![Arc::new(vec![1.0, 2.0, 3.0, 4.0])], backs: vec![None] };
    let y = g.rope(0, 1);
    assert_eq!(*g.vals[y], vec![1.0, 4.0, 3.0, 4.0]);
    let mut grads = Grads::default();
    (g.backs[y].as_ref().unwrap())(&Buf(vec![1.0, 1.0, 1.0, 1.0]), &mut grads);
    assert_eq!(grads.acc, vec![-1.0, -2.0, 1.0, 1.0]);
}
