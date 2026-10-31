//! The canonical encoding a node's key hashes: tagged little-endian words, with
//! -0.0 written as 0.0 so equal values hash equal.

use crate::{Seg, Shape, Sketch};
use makepad_core_util::sha256::Sha256;

pub(crate) struct Hasher(Sha256);

impl Hasher {
    pub fn new() -> Hasher {
        Hasher(Sha256::new())
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.0.update(b);
    }
    pub fn u(&mut self, v: u64) {
        self.0.update(&v.to_le_bytes());
    }
    pub fn f(&mut self, v: f64) {
        self.u(if v == 0.0 { 0 } else { v.to_bits() });
    }
    pub fn opt(&mut self, v: Option<f64>) {
        match v {
            Some(x) => {
                self.u(1);
                self.f(x);
            }
            None => self.u(0),
        }
    }
    pub fn p2(&mut self, p: [f64; 2]) {
        self.f(p[0]);
        self.f(p[1]);
    }
    pub fn p3(&mut self, p: [f64; 3]) {
        self.f(p[0]);
        self.f(p[1]);
        self.f(p[2]);
    }
    pub fn str(&mut self, s: &str) {
        self.u(s.len() as u64);
        self.0.update(s.as_bytes());
    }
    pub fn sketch(&mut self, s: &Sketch) {
        self.u(s.shapes.len() as u64);
        for shape in &s.shapes {
            match shape {
                Shape::Rect { center, size, corner, chamfer } => {
                    self.u(0);
                    self.p2(*center);
                    self.p2(*size);
                    self.f(*corner);
                    self.f(*chamfer);
                }
                Shape::Circle { center, r } => {
                    self.u(1);
                    self.p2(*center);
                    self.f(*r);
                }
                Shape::Ellipse { center, rx, ry } => {
                    self.u(2);
                    self.p2(*center);
                    self.f(*rx);
                    self.f(*ry);
                }
                Shape::Slot { a, b, r } => {
                    self.u(3);
                    self.p2(*a);
                    self.p2(*b);
                    self.f(*r);
                }
                Shape::Regular { center, r, sides, deg } => {
                    self.u(4);
                    self.p2(*center);
                    self.f(*r);
                    self.u(*sides as u64);
                    self.f(*deg);
                }
                Shape::Polygon { points, fillet } => {
                    self.u(5);
                    self.u(points.len() as u64);
                    points.iter().for_each(|p| self.p2(*p));
                    self.f(*fillet);
                }
                Shape::Path { start, segs } => {
                    self.u(6);
                    self.p2(*start);
                    self.u(segs.len() as u64);
                    for s in segs {
                        self.seg(s);
                    }
                }
            }
        }
    }
    fn seg(&mut self, s: &Seg) {
        match s {
            Seg::Line(p) => {
                self.u(0);
                self.p2(*p);
            }
            Seg::Rel(p) => {
                self.u(1);
                self.p2(*p);
            }
            Seg::H(d) => {
                self.u(2);
                self.f(*d);
            }
            Seg::V(d) => {
                self.u(3);
                self.f(*d);
            }
            Seg::Polar { len, deg } => {
                self.u(4);
                self.f(*len);
                self.f(*deg);
            }
            Seg::ArcThrough { through, to } => {
                self.u(5);
                self.p2(*through);
                self.p2(*to);
            }
            Seg::ArcRadius { to, r, large } => {
                self.u(6);
                self.p2(*to);
                self.f(*r);
                self.u(*large as u64);
            }
            Seg::Tangent(p) => {
                self.u(7);
                self.p2(*p);
            }
            Seg::Spline(points) => {
                self.u(8);
                self.u(points.len() as u64);
                points.iter().for_each(|p| self.p2(*p));
            }
            Seg::Fillet(r) => {
                self.u(9);
                self.f(*r);
            }
            Seg::Chamfer(d) => {
                self.u(10);
                self.f(*d);
            }
        }
    }
    pub fn finish(self) -> [u8; 32] {
        self.0.finish()
    }
}
