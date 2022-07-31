use std::cell::RefCell;
use std::rc::Rc;
use crate::makepad_math::{Rect, Vec4, Vec2};

#[derive(Clone, Default)]
pub struct DebugInner {
    pub rects: Vec<(Rect, Vec4)>,
    pub points: Vec<(Vec2, Vec4)>,
    pub labels: Vec<(Vec2, Vec4, String)>,
}

#[derive(Clone, Default)]
pub struct Debug(Rc<RefCell<DebugInner >>);

impl Debug {
    const R: Vec4 = Vec4 {x: 1.0, y: 0.0, z: 0.0, w: 1.0};
    const G: Vec4 = Vec4 {x: 0.0, y: 1.0, z: 0.0, w: 1.0};
    const B: Vec4 = Vec4 {x: 0.0, y: 0.0, z: 1.0, w: 1.0};
    
    pub fn p(&self, color: Vec4, p: Vec2) {
        let mut inner = self.0.borrow_mut();
        inner.points.push((p, color));
    }
    
    pub fn pr(&self, p: Vec2) {self.p(Self::R, p)}
    pub fn pg(&self, p: Vec2) {self.p(Self::G, p)}
    pub fn pb(&self, p: Vec2) {self.p(Self::B, p)}

    pub fn pl(&self, color: Vec4, p: Vec2, label:String) {
        let mut inner = self.0.borrow_mut();
        inner.labels.push((p, color,label));
    }
    
    pub fn lr(&self, p: Vec2, label:String) {self.pl(Self::R, p, label)}
    pub fn lg(&self, p: Vec2, label:String) {self.pl(Self::G, p, label)}
    pub fn lb(&self, p: Vec2, label:String) {self.pl(Self::B, p, label)}

    
    pub fn r(&self, color: Vec4, p: Rect) {
        let mut inner = self.0.borrow_mut();
        inner.rects.push((p, color));
    }
    pub fn rr(&self, r: Rect) {self.r(Self::R, r)}
    pub fn rg(&self, r: Rect) {self.r(Self::G, r)}
    pub fn rb(&self, r: Rect) {self.r(Self::B, r)}
    
    pub fn has_data(&self)->bool{
        let inner = self.0.borrow();
        !inner.points.is_empty() || !inner.rects.is_empty() || !inner.labels.is_empty()
    }
    
    pub fn take_rects(&self)->Vec<(Rect, Vec4)>{
        let mut inner = self.0.borrow_mut();
        let mut swap = Vec::new();
        std::mem::swap(&mut swap, &mut inner.rects);
        swap
    }

    pub fn take_points(&self)->Vec<(Vec2, Vec4)>{
        let mut inner = self.0.borrow_mut();
        let mut swap = Vec::new();
        std::mem::swap(&mut swap, &mut inner.points);
        swap
    }

    pub fn take_labels(&self)->Vec<(Vec2, Vec4, String)>{
        let mut inner = self.0.borrow_mut();
        let mut swap = Vec::new();
        std::mem::swap(&mut swap, &mut inner.labels);
        swap
    }
}
