use serde::*; 

#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect{
    pub x:f32,
    pub y:f32,
    pub w:f32,
    pub h:f32
}

impl Rect{
    pub fn zero()->Rect{
        Rect{x:0.0,y:0.0,w:0.0,h:0.0}
    }
    pub fn contains(&self, x:f32, y:f32)->bool{
        return x >= self.x && x <= self.x + self.w &&
            y >= self.y && y <= self.y + self.h;
    }
    pub fn intersects(&self, r:Rect)->bool{
        !(
            r.x > self.x + self.w || 
            r.x + r.w < self.x || 
            r.y > self.y + self.h ||
            r.y + r.h < self.y
        )
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Mat4{
    pub v: [f32; 16],
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vec2{
    pub x: f32,
    pub y: f32,
}

impl Vec2{
    pub fn zero()->Vec2{
        Vec2{x:0.0,y:0.0}
    }

    pub fn distance(&self, other:&Vec2)->f32{
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx*dx+dy*dy).sqrt()
    }
}
/*
pub fn vec2(x:f32, y:f32)->Vec2{
    Vec2{x:x, y:y}
}*/

#[derive(Clone, Copy, Default, Debug)]
pub struct Vec3{
    pub x: f32,
    pub y: f32,
    pub z: f32
}

impl Vec3{
    pub fn zero()->Vec3{
        Vec3{x:0.0,y:0.0,z:0.0}
    }
}

/*
pub fn vec3(x:f32, y:f32, z:f32)->Vec3{
    Vec3{x:x, y:y, z:z}
}*/

#[derive(Clone, Copy, Default, Debug)]
pub struct Vec4{
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32
}

impl Vec4{
    pub fn zero()->Vec4{
        Vec4{x:0.0,y:0.0,z:0.0,w:0.0}
    }
}


#[derive(Clone, Copy, Default, Debug)]
pub struct Color{
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32
}

impl Color{
    pub fn zero()->Color{
        Color{r:0.0, g:0.0, b:0.0, a:0.0}
    }
}


/*
pub fn vec4(x:f32, y:f32, z:f32, w:f32)->Vec4{
    Vec4{x:x, y:y, z:z, w:w}
}*/


impl Mat4{
    pub fn identity() -> Mat4{
        return Mat4{v:[
            1.0,0.0,0.0,0.0,
            0.0,1.0,0.0,0.0,
            0.0,0.0,1.0,0.0,
            0.0,0.0,0.0,1.0
        ]}
    }

    pub fn rotate_tsrt(t1: Vec3, s: Vec3, r: Vec3, t2: Vec3) -> Mat4{
        let cx = f32::cos(r.x);
        let cy = f32::cos(r.y);
        let cz = f32::cos(r.z);
        let sx = f32::sin(r.x);
        let sy = f32::sin(r.y);
        let sz = f32::sin(r.z);
        let m0 = s.x * (cy * cz + sx * sy * sz);
        let m1 = s.y * (-sz * cy + cz * sx * sy);
        let m2 = s.z * (sy * cx);
        let m4 = s.x * (sz * cx);
        let m5 = s.y * (cx * cz);
        let m6 = s.z * (-sx);
        let m8 = s.x * (-sy * cz + cy * sx * sz);
        let m9 = s.y * (sy * sz + cy * sx * cz);
        let m10 = s.z * (cx * cy);
        return Mat4{v:[
            m0, m4, m8, 0.0,
            m1, m5, m9, 0.0,
            m2, m6, m10, 0.0,
            t2.x + (m0 * t1.x + m1 * t1.y + m1 * t1.z),
            t2.y + (m4 * t1.x + m5 * t1.y + m6 * t1.z),
            t2.z + (m8 * t1.x + m9 * t1.y + m10 * t1.z),
            1.0
        ]}
    }

    pub fn perspective(fov_y:f32, aspect:f32, near:f32, far:f32) -> Mat4{
        let f = 1.0 / f32::tan(fov_y / 2.0);
        let nf = 1.0 / (near - far);
        return Mat4{v:[
            f / aspect, 0.0, 0.0, 0.0,
            0.0, f , 0.0, 0.0,
            0.0, 0.0, (far + near) * nf, -1.0,
            0.0, 0.0, (2.0 * far * near) * nf, 0.0
        ]}
    }

    pub fn scale_translate(sx:f32, sy:f32, sz:f32, x:f32, y:f32, z:f32)->Mat4{
        return Mat4{v:[
            sx,0.0,0.0,0.0,
            0.0,sy,0.0,0.0,
            0.0,0.0,sz,0.0,
            x,y,z,1.0
        ]}

    }

    pub fn ortho(left:f32, right:f32, top:f32, bottom:f32, near:f32, far:f32, scalex:f32, scaley:f32) -> Mat4{
        let lr = 1.0 / (left - right);
        let bt = 1.0 / (bottom - top);
        let nf = 1.0 / (near - far);
        /*return Mat4{v:[
            -2.0 * lr * scalex, 0.0, 0.0, (left+right) * lr,
            0.0, -2.0 * bt * scaley, 0.0, (top+bottom) * bt,
            0.0, 0.0, 2.0 * nf, (far + near) * nf,
            0.0, 0.0, 0.0, 1.0
        ]}*/
        return Mat4{v:[
            -2.0 * lr * scalex, 0.0, 0.0, 0.0,
            0.0, -2.0 * bt * scaley, 0.0, 0.0,
            0.0, 0.0, -1.0 * nf, 0.0,
            (left+right) * lr, (top+bottom) * bt,  0.5+(far+near)*nf, 1.0
        ]}
    }
    
    pub fn transform_vec4(&self, v:Vec4)->Vec4{
        let m = &self.v;
        Vec4{
            x:m[0] * v.x + m[4] * v.y + m[8] * v.z + m[12] * v.w,
            y:m[1] * v.x + m[5] * v.y + m[9] * v.z + m[13] * v.w,
            z:m[2] * v.x + m[6] * v.y + m[10] * v.z + m[14] * v.w,
            w:m[3] * v.x + m[7] * v.y + m[11] * v.z + m[15] * v.w
        }
    }
} 
