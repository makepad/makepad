use crate::cx::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeometryGen {
    pub vertices: Vec<f32>, // vec4 pos, vec3 normal, vec2 uv
    pub indices: Vec<u32>
}

#[derive(Clone, Copy)]
pub enum GeometryAxis {
    X = 0,
    Y = 1,
    Z = 2,
}

impl GeometryGen {
    
    pub fn to_geometry(self, cx:&mut Cx, geometry:Geometry){
        let cxgeom = &mut cx.geometries[geometry.geometry_id];
        cxgeom.indices = self.indices;
        cxgeom.vertices = self.vertices;
        cxgeom.dirty = true;
    }
    
    pub fn from_quad_2d(x1: f32, y1: f32, x2: f32, y2: f32) -> GeometryGen {
        let mut g = Self::default();
        g.add_quad_2d(x1, y1, x2, y2);
        g
    }
    
    pub fn from_cube_3d(
        width: f32,
        height: f32,
        depth: f32,
        width_segments: usize,
        height_segments: usize,
        depth_segments: usize
    ) -> GeometryGen {
        let mut g = Self::default();
        g.add_cube_3d(width, height, depth, width_segments, height_segments, depth_segments);
        g
    }
    
    // requires pos:vec2 normalized layout
    pub fn add_quad_2d(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) {
        let vertex_offset = self.vertices.len() as u32;
        self.vertices.push(x1);
        self.vertices.push(y1);
        self.vertices.push(x2);
        self.vertices.push(y1);
        self.vertices.push(x2);
        self.vertices.push(y2);
        self.vertices.push(x1);
        self.vertices.push(y2);
        self.indices.push(vertex_offset + 0);
        self.indices.push(vertex_offset + 1);
        self.indices.push(vertex_offset + 2);
        self.indices.push(vertex_offset + 2);
        self.indices.push(vertex_offset + 3);
        self.indices.push(vertex_offset + 0);
    }
    
    // requires pos:vec3, id:float, normal:vec3, uv:vec2 layout
    pub fn add_cube_3d(
        &mut self,
        width: f32,
        height: f32,
        depth: f32,
        width_segments: usize,
        height_segments: usize,
        depth_segments: usize
    ) {
        self.add_plane_3d(GeometryAxis::Z, GeometryAxis::Y, GeometryAxis::X, -1.0, -1.0, depth, height, width, depth_segments, height_segments, 0.0);
        self.add_plane_3d(GeometryAxis::Z, GeometryAxis::Y, GeometryAxis::X, 1.0, -1.0, depth, height, -width, depth_segments, height_segments, 1.0);
        self.add_plane_3d(GeometryAxis::X, GeometryAxis::Z, GeometryAxis::Y, 1.0, 1.0, width, depth, height, width_segments, depth_segments, 2.0);
        self.add_plane_3d(GeometryAxis::X, GeometryAxis::Z, GeometryAxis::Y, 1.0, -1.0, width, depth, -height, width_segments, depth_segments, 3.0);
        self.add_plane_3d(GeometryAxis::X, GeometryAxis::Y, GeometryAxis::Z, 1.0, -1.0, width, height, depth, width_segments, height_segments, 4.0);
        self.add_plane_3d(GeometryAxis::X, GeometryAxis::Y, GeometryAxis::Z, -1.0, -1.0, width, height, -depth, width_segments, height_segments, 5.0);
    }
    
    
    // requires pos:vec3, id:float, normal:vec3, uv:vec2 layout
    pub fn add_plane_3d(
        &mut self,
        u: GeometryAxis,
        v: GeometryAxis,
        w: GeometryAxis,
        udir: f32,
        vdir: f32,
        width: f32,
        height: f32,
        depth: f32,
        grid_x: usize,
        grid_y: usize,
        id: f32
    ) {
        let segment_width = width / (grid_x as f32);
        let segment_height = height / (grid_y as f32);
        let width_half = width / 2.0;
        let height_half = height / 2.0;
        let depth_half = depth / 2.0;
        let grid_x1 = grid_x + 1;
        let grid_y1 = grid_y + 1;
        
        let vertex_offset = self.vertices.len() / 9;
        
        for iy in 0..grid_y1 {
            let y = (iy as f32) * segment_height - height_half;
            
            for ix in 0..grid_x1 {
                
                let x = (ix as f32) * segment_width - width_half;
                let off = self.vertices.len();
                self.vertices.push(0.0);
                self.vertices.push(0.0);
                self.vertices.push(0.0);
                
                self.vertices[off + u as usize] = x * udir;
                self.vertices[off + v as usize] = y * vdir;
                self.vertices[off + w as usize] = depth_half;
                
                self.vertices.push(id);
                let off = self.vertices.len();
                self.vertices.push(0.0);
                self.vertices.push(0.0);
                self.vertices.push(0.0);
                self.vertices[off + w as usize] = if depth > 0.0 {1.0} else {-1.0};
                
                self.vertices.push((ix as f32) / (grid_x as f32));
                self.vertices.push(1.0 - (iy as f32) / (grid_y as f32));
            }
        }
        
        for iy in 0..grid_y {
            for ix in 0..grid_x {
                let a = vertex_offset + ix + grid_x1 * iy;
                let b = vertex_offset + ix + grid_x1 * (iy + 1);
                let c = vertex_offset + (ix + 1) + grid_x1 * (iy + 1);
                let d = vertex_offset + (ix + 1) + grid_x1 * iy;
                self.indices.push(a as u32);
                self.indices.push(b as u32);
                self.indices.push(d as u32);
                self.indices.push(b as u32);
                self.indices.push(c as u32);
                self.indices.push(d as u32);
            }
        }
    }
}

live_body!{
    GeometryQuad2D: Geometry {
        rust_type: {{GeometryQuad2D}};
        x1: 0.0,
        y1: 0.0,
        x2: 1.0,
        y2: 1.0,
    }
}

impl GeometryFields for GeometryQuad2D {
    fn geometry_fields(&self, fields: &mut Vec<GeometryField>) {
        fields.push(GeometryField {id: id!(geom_pos), ty: Ty::Vec2});
    }
    
    fn get_geometry(&self) -> Geometry {
        self.geometry
    }
    
    fn live_type_check(&self) -> LiveType {
        Self::live_type()
    }
}

pub struct GeometryQuad2D {
    //#[private()]
    pub live_ptr: Option<LivePtr>,
    //#[default(0.0)]
    pub x1: f32,
    //#[default(0.0)]
    pub y1: f32,
    //#[default(1.0)]
    pub x2: f32,
    //#[default(1.0)]
    pub y2: f32,
    //#[private()]
    pub geometry: Geometry
}

impl GeometryQuad2D {
    fn live_update_value(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {
        match id {
            id!(x1) => self.x1.live_update(cx, ptr),
            id!(y1) => self.y1.live_update(cx, ptr),
            id!(x2) => self.x2.live_update(cx, ptr),
            id!(y2) => self.y2.live_update(cx, ptr),
            _ => ()
        }
    }
}

impl LiveUpdate for GeometryQuad2D {
    fn live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr) {
        // how do we verify this?
        if let Some(mut iter) = cx.shader_registry.live_registry.live_class_iterator(live_ptr) {
            while let Some((id, live_ptr)) = iter.next(&cx.shader_registry.live_registry) {
                if id == id!(rust_type) && !cx.verify_type_signature(live_ptr, Self::live_type()) {
                    // give off an error/warning somehow!
                    return;
                }
                self.live_update_value(cx, id, live_ptr)
            }
        }
        // lets generate geometry
        GeometryGen::from_quad_2d(
            self.x1,
            self.y1,
            self.x2,
            self.y2,
        ).to_geometry(cx, self.geometry);
    }
    
    fn _live_type(&self) -> LiveType {
        Self::live_type()
    }
}

impl LiveNew for GeometryQuad2D {
    fn live_new(cx: &mut Cx) -> Self {
        Self {
            live_ptr: None,
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            geometry: cx.new_geometry()
        }
    }
    
    fn live_type() -> LiveType {
        LiveType(std::any::TypeId::of::<GeometryQuad2D>())
    }
    
    fn live_register(cx: &mut Cx) {
        cx.register_live_body(live_body());
        struct Factory();
        impl LiveFactory for Factory {
            fn live_new(&self, cx: &mut Cx) -> Box<dyn LiveUpdate> {
                Box::new(GeometryQuad2D ::live_new(cx))
            }
            
            fn live_fields(&self, fields: &mut Vec<LiveField>) {
                fields.push(LiveField {id: id!(x1), live_type: f32::live_type()});
                fields.push(LiveField {id: id!(y1), live_type: f32::live_type()});
                fields.push(LiveField {id: id!(x2), live_type: f32::live_type()});
                fields.push(LiveField {id: id!(y2), live_type: f32::live_type()});
            }
            
            fn live_type(&self) -> LiveType where Self: Sized {
                GeometryQuad2D::live_type()
            }
        }
        cx.live_factories.insert(GeometryQuad2D::live_type(), Box::new(Factory()));
    }
}
