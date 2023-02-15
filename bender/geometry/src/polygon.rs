use crate::linear_path::Command;
use crate::{LineSegment, Point};
use std::slice::Iter;

#[derive(Clone, Debug, PartialEq)]
pub struct Polygon {
    pub vertices: Vec<Point>,
}

impl Polygon {
    pub fn commands(&self) -> Vec<Command> {
        let mut commands = Vec::new();
        commands.push(Command::MoveTo(self.vertices.first().cloned().unwrap()));
        for vertex in self.vertices[1..].iter().cloned() {
            commands.push(Command::LineTo(vertex));
        }
        commands.push(Command::Close);
        commands
    }

    pub fn edges(&self) -> Edges {
        Edges {
            first_vertex: self.vertices.first(),
            vertex_iter: self.vertices[1..].iter(),
            last_vertex: self.vertices.first(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Edges<'a> {
    first_vertex: Option<&'a Point>,
    vertex_iter: Iter<'a, Point>,
    last_vertex: Option<&'a Point>,
}

impl<'a> Iterator for Edges<'a> {
    type Item = LineSegment;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(vertex) = self.vertex_iter.next() {
            let first_vertex = self.first_vertex.replace(vertex).unwrap();
            return Some(LineSegment::new(*first_vertex, *vertex));
        }
        if let Some(vertex) = self.last_vertex.take() {
            let first_vertex = self.first_vertex.take().unwrap();
            return Some(LineSegment::new(*first_vertex, *vertex));
        }
        None
    }
}
