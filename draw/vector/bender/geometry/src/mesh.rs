pub trait Callbacks {
    fn vertex(&mut self, x: f32, y: f32) -> u16;

    fn triangle(&mut self, index_0: u16, index_1: u16, index_2: u16);
}

#[derive(Clone, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u16>,
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 2],
}

#[derive(Debug)]
pub struct Writer<'a> {
    pub vertices: &'a mut Vec<Vertex>,
    pub indices: &'a mut Vec<u16>,
}

impl<'a> Writer<'a> {
    pub fn new(mesh: &'a mut Mesh) -> Self {
        Self {
            vertices: &mut mesh.vertices,
            indices: &mut mesh.indices,
        }
    }
}

impl<'a> Callbacks for Writer<'a> {
    fn vertex(&mut self, x: f32, y: f32) -> u16 {
        let vertex = Vertex { position: [x, y] };
        let index = self.vertices.len() as u16;
        self.vertices.push(vertex);
        index
    }

    fn triangle(&mut self, index_0: u16, index_1: u16, index_2: u16) {
        self.indices.extend(&[index_0, index_1, index_2]);
    }
}
