use crate::{LineSegment, Point};
use std::slice::Iter;

#[derive(Clone, Debug, PartialEq)]
pub struct Polyline {
    pub vertices: Vec<Point>,
}

impl Polyline {
    pub fn edges(&self) -> Edges {
        Edges {
            first_vertex: self.vertices.first(),
            vertices_iter: self.vertices[1..self.vertices.len() - 1].iter(),
            last_vertex: self.vertices.last(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Edges<'a> {
    first_vertex: Option<&'a Point>,
    vertices_iter: Iter<'a, Point>,
    last_vertex: Option<&'a Point>,
}

impl<'a> Iterator for Edges<'a> {
    type Item = LineSegment;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(vertex) = self.vertices_iter.next() {
            let first_vertex = self.first_vertex.replace(vertex).unwrap();
            return Some(LineSegment::new(*first_vertex, *vertex));
        }
        if let Some(last_vertex) = self.last_vertex.take() {
            let first_vertex = self.first_vertex.take().unwrap();
            return Some(LineSegment::new(*first_vertex, *last_vertex));
        }
        None
    }
}

impl<'a> DoubleEndedIterator for Edges<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if let Some(vertex) = self.vertices_iter.next_back() {
            let last_vertex = self.last_vertex.replace(vertex).unwrap();
            return Some(LineSegment::new(*vertex, *last_vertex));
        }
        if let Some(first_vertex) = self.first_vertex.take() {
            let last_vertex = self.last_vertex.take().unwrap();
            return Some(LineSegment::new(*first_vertex, *last_vertex));
        }
        None
    }
}
