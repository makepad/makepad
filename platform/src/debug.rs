use crate::area::Area;
use crate::cx::Cx;
use crate::makepad_math::{Rect, Vec2d, Vec4f};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Default)]
pub struct DebugInner {
    pub areas: Vec<(Area, Vec2d, Vec2d, Vec4f)>,
    pub rects: Vec<(Rect, Vec4f)>,
    pub points: Vec<(Vec2d, Vec4f)>,
    pub labels: Vec<(Vec2d, Vec4f, String)>,
    pub marker: u64,
}

#[derive(Clone, Default)]
pub struct Debug(Rc<RefCell<DebugInner>>);

impl Debug {
    pub fn marker(&self) -> u64 {
        let inner = self.0.borrow();
        inner.marker
    }

    pub fn set_marker(&mut self, v: u64) {
        let mut inner = self.0.borrow_mut();
        inner.marker = v
    }

    pub fn point(&self, p: Vec2d, color: Vec4f) {
        let mut inner = self.0.borrow_mut();
        inner.points.push((p, color));
    }

    pub fn label(&self, p: Vec2d, color: Vec4f, label: String) {
        let mut inner = self.0.borrow_mut();
        inner.labels.push((p, color, label));
    }

    pub fn rect(&self, p: Rect, color: Vec4f) {
        let mut inner = self.0.borrow_mut();
        inner.rects.push((p, color));
    }

    pub fn area(&self, area: Area, color: Vec4f) {
        let mut inner = self.0.borrow_mut();
        inner
            .areas
            .push((area, Vec2d::default(), Vec2d::default(), color));
    }

    pub fn area_offset(&self, area: Area, tl: Vec2d, br: Vec2d, color: Vec4f) {
        let mut inner = self.0.borrow_mut();
        inner.areas.push((area, tl, br, color));
    }

    pub fn has_data(&self) -> bool {
        let inner = self.0.borrow();
        !inner.points.is_empty()
            || !inner.rects.is_empty()
            || !inner.labels.is_empty()
            || !inner.areas.is_empty()
    }

    pub fn take_rects(&self) -> Vec<(Rect, Vec4f)> {
        let mut inner = self.0.borrow_mut();
        let mut swap = Vec::new();
        std::mem::swap(&mut swap, &mut inner.rects);
        swap
    }

    pub fn take_points(&self) -> Vec<(Vec2d, Vec4f)> {
        let mut inner = self.0.borrow_mut();
        let mut swap = Vec::new();
        std::mem::swap(&mut swap, &mut inner.points);
        swap
    }

    pub fn take_labels(&self) -> Vec<(Vec2d, Vec4f, String)> {
        let mut inner = self.0.borrow_mut();
        let mut swap = Vec::new();
        std::mem::swap(&mut swap, &mut inner.labels);
        swap
    }

    pub fn take_areas(&self) -> Vec<(Area, Vec2d, Vec2d, Vec4f)> {
        let mut inner = self.0.borrow_mut();
        let mut swap = Vec::new();
        std::mem::swap(&mut swap, &mut inner.areas);
        swap
    }
}

impl Cx {
}
