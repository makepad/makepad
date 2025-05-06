use crate::Primitive;
use bender_geometry::{Mesh, Polygon, Polyline};

pub trait Draw {
    fn draw(&self, options: Options);
}

impl Draw for Mesh {
    fn draw(
        &self,
        Options {
            triangle_color,
            vertex_color,
            ..
        }: Options,
    ) {
        let mut triangles = Primitive::begin(gl::TRIANGLES);
        triangles.color(
            triangle_color[0],
            triangle_color[1],
            triangle_color[2],
            triangle_color[3],
        );
        for triangle in self.indices.chunks(3) {
            for index in triangle.iter().cloned() {
                let vertex = self.vertices[index as usize];
                triangles.vertex(vertex.position[0], vertex.position[1]);
            }
        }
        triangles.end();
        let mut vertices = Primitive::begin(gl::POINTS);
        vertices.color(
            vertex_color[0],
            vertex_color[1],
            vertex_color[2],
            vertex_color[3],
        );
        for vertex in self.vertices.iter().cloned() {
            vertices.vertex(vertex.position[0], vertex.position[1]);
        }
        vertices.end();
    }
}

impl Draw for Polygon {
    fn draw(
        &self,
        Options {
            edge_color,
            vertex_color,
            ..
        }: Options,
    ) {
        let mut edges = Primitive::begin(gl::LINE_LOOP);
        edges.color(edge_color[0], edge_color[1], edge_color[2], edge_color[3]);
        for vertex in self.vertices.iter().cloned() {
            edges.vertex(vertex.x(), vertex.y());
        }
        edges.end();
        let mut vertices = Primitive::begin(gl::POINTS);
        vertices.color(
            vertex_color[0],
            vertex_color[1],
            vertex_color[2],
            vertex_color[3],
        );
        for vertex in self.vertices.iter().cloned() {
            vertices.vertex(vertex.x(), vertex.y());
        }
        vertices.end();
    }
}

impl Draw for Polyline {
    fn draw(
        &self,
        Options {
            edge_color,
            vertex_color,
            ..
        }: Options,
    ) {
        let mut edges = Primitive::begin(gl::LINE_STRIP);
        edges.color(edge_color[0], edge_color[1], edge_color[2], edge_color[3]);
        for vertex in self.vertices.iter().cloned() {
            edges.vertex(vertex.x(), vertex.y());
        }
        edges.end();
        let mut vertices = Primitive::begin(gl::POINTS);
        vertices.color(
            vertex_color[0],
            vertex_color[1],
            vertex_color[2],
            vertex_color[3],
        );
        for vertex in self.vertices.iter().cloned() {
            vertices.vertex(vertex.x(), vertex.y());
        }
        vertices.end();
    }
}

#[derive(Clone, Copy)]
pub struct Options {
    pub triangle_color: [f32; 4],
    pub edge_color: [f32; 4],
    pub vertex_color: [f32; 4],
}

impl Default for Options {
    fn default() -> Self {
        Self {
            triangle_color: [0.5, 0.5, 1.0, 1.0],
            edge_color: [0.5, 1.0, 0.5, 1.0],
            vertex_color: [1.0, 0.5, 0.5, 1.0],
        }
    }
}
