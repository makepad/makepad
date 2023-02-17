use crate::constants::*;
use gl;
use gl::types::{GLenum, GLsizei, GLsizeiptr};
use std::ffi::c_void;
use std::mem;

#[derive(Clone, Debug)]
pub struct Primitive {
    mode: GLenum,
    current_color: Color,
    vertices: Vec<Vertex>,
}

impl Primitive {
    pub fn begin(mode: GLenum) -> Self {
        Self {
            mode,
            current_color: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            vertices: Vec::new(),
        }
    }

    pub fn end(self) {
        unsafe {
            let mut vertex_array = mem::uninitialized();
            gl::GenVertexArrays(1, &mut vertex_array);
            gl::BindVertexArray(vertex_array);
            let mut buffer = mem::uninitialized();
            gl::GenBuffers(1, &mut buffer);
            gl::BindBuffer(gl::ARRAY_BUFFER, buffer);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                (self.vertices.len() * mem::size_of::<Vertex>()) as GLsizeiptr,
                self.vertices.as_ptr() as *const c_void,
                gl::STATIC_DRAW,
            );
            gl::VertexAttribPointer(
                POSITION_ATTRIBUTE,
                2,
                gl::FLOAT,
                gl::FALSE,
                mem::size_of::<Vertex>() as GLsizei,
                &(*(0 as *const Vertex)).position as *const _ as *const c_void,
            );
            gl::VertexAttribPointer(
                COLOR_ATTRIBUTE,
                4,
                gl::FLOAT,
                gl::FALSE,
                mem::size_of::<Vertex>() as GLsizei,
                &(*(0 as *const Vertex)).current_color as *const _ as *const c_void,
            );
            gl::EnableVertexAttribArray(POSITION_ATTRIBUTE);
            gl::EnableVertexAttribArray(COLOR_ATTRIBUTE);
            gl::DrawArrays(self.mode, 0, self.vertices.len() as GLsizei);
            gl::DeleteBuffers(1, &buffer);
            gl::DeleteVertexArrays(1, &vertex_array);
        }
    }

    pub fn color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.current_color = Color { r, g, b, a };
    }

    pub fn vertex(&mut self, x: f32, y: f32) {
        self.vertices.push(Vertex {
            position: Position { x, y },
            current_color: self.current_color,
        })
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct Vertex {
    position: Position,
    current_color: Color,
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct Position {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct Color {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}
