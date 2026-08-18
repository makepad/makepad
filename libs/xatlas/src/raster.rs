//! Conservative triangle raster from `vendor/xatlas.cpp:4687`.

use crate::math::*;

pub type SamplingCallback = fn(*mut (), i32, i32) -> bool;

struct ClippedTriangle {
    vertices_a: [Vec2; 8],
    vertices_b: [Vec2; 8],
    num_vertices: u32,
    active_vertex_buffer: u32,
    area: f32,
}

impl ClippedTriangle {
    fn new(a: Vec2, b: Vec2, c: Vec2) -> Self {
        let mut t = Self {
            vertices_a: [Vec2::splat(0.0); 8],
            vertices_b: [Vec2::splat(0.0); 8],
            num_vertices: 3,
            active_vertex_buffer: 0,
            area: 0.0,
        };
        t.vertices_a[0] = a;
        t.vertices_a[1] = b;
        t.vertices_a[2] = c;
        t
    }

    fn buf(&self, which: u32) -> &[Vec2; 8] {
        if which == 0 {
            &self.vertices_a
        } else {
            &self.vertices_b
        }
    }

    fn buf_mut(&mut self, which: u32) -> &mut [Vec2; 8] {
        if which == 0 {
            &mut self.vertices_a
        } else {
            &mut self.vertices_b
        }
    }

    fn clip_horizontal_plane(&mut self, offset: f32, clipdirection: f32) {
        let src = self.active_vertex_buffer;
        let dst = src ^ 1;
        {
            let v = if src == 0 {
                &mut self.vertices_a
            } else {
                &mut self.vertices_b
            };
            v[self.num_vertices as usize] = v[0];
        }
        let mut dy1 = offset
            - if src == 0 {
                self.vertices_a[0].y
            } else {
                self.vertices_b[0].y
            };
        let mut dy1in = clipdirection * dy1 >= 0.0;
        let mut p = 0u32;
        for k in 0..self.num_vertices {
            let (vk, vk1) = if src == 0 {
                (self.vertices_a[k as usize], self.vertices_a[k as usize + 1])
            } else {
                (self.vertices_b[k as usize], self.vertices_b[k as usize + 1])
            };
            let dy2 = offset - vk1.y;
            let dy2in = clipdirection * dy2 >= 0.0;
            let dst_buf = if dst == 0 {
                &mut self.vertices_a
            } else {
                &mut self.vertices_b
            };
            if dy1in {
                dst_buf[p as usize] = vk;
                p += 1;
            }
            if (dy1in as i32) + (dy2in as i32) == 1 {
                let dx = vk1.x - vk.x;
                let dy = vk1.y - vk.y;
                dst_buf[p as usize] = Vec2::new(vk.x + dy1 * (dx / dy), offset);
                p += 1;
            }
            dy1 = dy2;
            dy1in = dy2in;
        }
        self.num_vertices = p;
        self.active_vertex_buffer = dst;
    }

    fn clip_vertical_plane(&mut self, offset: f32, clipdirection: f32) {
        let src = self.active_vertex_buffer;
        let dst = src ^ 1;
        {
            let v = if src == 0 {
                &mut self.vertices_a
            } else {
                &mut self.vertices_b
            };
            v[self.num_vertices as usize] = v[0];
        }
        let mut dx1 = offset
            - if src == 0 {
                self.vertices_a[0].x
            } else {
                self.vertices_b[0].x
            };
        let mut dx1in = clipdirection * dx1 >= 0.0;
        let mut p = 0u32;
        for k in 0..self.num_vertices {
            let (vk, vk1) = if src == 0 {
                (self.vertices_a[k as usize], self.vertices_a[k as usize + 1])
            } else {
                (self.vertices_b[k as usize], self.vertices_b[k as usize + 1])
            };
            let dx2 = offset - vk1.x;
            let dx2in = clipdirection * dx2 >= 0.0;
            let dst_buf = if dst == 0 {
                &mut self.vertices_a
            } else {
                &mut self.vertices_b
            };
            if dx1in {
                dst_buf[p as usize] = vk;
                p += 1;
            }
            if (dx1in as i32) + (dx2in as i32) == 1 {
                let dx = vk1.x - vk.x;
                let dy = vk1.y - vk.y;
                dst_buf[p as usize] = Vec2::new(offset, vk.y + dx1 * (dy / dx));
                p += 1;
            }
            dx1 = dx2;
            dx1in = dx2in;
        }
        self.num_vertices = p;
        self.active_vertex_buffer = dst;
    }

    fn compute_area(&mut self) {
        let src = self.active_vertex_buffer;
        {
            let v = if src == 0 {
                &mut self.vertices_a
            } else {
                &mut self.vertices_b
            };
            v[self.num_vertices as usize] = v[0];
        }
        self.area = 0.0;
        for k in 0..self.num_vertices {
            let (vk, vk1) = if src == 0 {
                (self.vertices_a[k as usize], self.vertices_a[k as usize + 1])
            } else {
                (self.vertices_b[k as usize], self.vertices_b[k as usize + 1])
            };
            let f = vk.x * vk1.y - vk1.x * vk.y;
            self.area += f;
        }
        self.area = 0.5 * self.area.abs();
    }

    fn clip_aa_box(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.clip_vertical_plane(x0, -1.0);
        self.clip_horizontal_plane(y0, -1.0);
        self.clip_vertical_plane(x1, 1.0);
        self.clip_horizontal_plane(y1, 1.0);
        self.compute_area();
    }
}

struct Triangle {
    v1: Vec2,
    v2: Vec2,
    v3: Vec2,
    n1: Vec2,
    n2: Vec2,
    n3: Vec2,
}

impl Triangle {
    fn new(v0: Vec2, v1: Vec2, v2: Vec2) -> Self {
        // xatlas.cpp:4793 — constructor stores v1=_v0, v2=_v2, v3=_v1
        let mut t = Self {
            v1: v0,
            v2,
            v3: v1,
            n1: Vec2::splat(0.0),
            n2: Vec2::splat(0.0),
            n3: Vec2::splat(0.0),
        };
        t.flip_backface();
        if t.is_valid() {
            t.compute_unit_inward_normals();
        }
        t
    }

    fn is_valid(&self) -> bool {
        let e0 = self.v3 - self.v1;
        let e1 = self.v2 - self.v1;
        let area = e0.y * e1.x - e1.y * e0.x;
        area != 0.0
    }

    fn flip_backface(&mut self) {
        if (self.v3.x - self.v1.x) * (self.v2.y - self.v1.y)
            - (self.v3.y - self.v1.y) * (self.v2.x - self.v1.x)
            < 0.0
        {
            std::mem::swap(&mut self.v1, &mut self.v2);
        }
    }

    fn compute_unit_inward_normals(&mut self) {
        self.n1 = self.v1 - self.v2;
        self.n1 = Vec2::new(-self.n1.y, self.n1.x);
        self.n1 = self.n1 * (1.0 / c_sqrt(dot2(self.n1, self.n1)));
        self.n2 = self.v2 - self.v3;
        self.n2 = Vec2::new(-self.n2.y, self.n2.x);
        self.n2 = self.n2 * (1.0 / c_sqrt(dot2(self.n2, self.n2)));
        self.n3 = self.v3 - self.v1;
        self.n3 = Vec2::new(-self.n3.y, self.n3.x);
        self.n3 = self.n3 * (1.0 / c_sqrt(dot2(self.n3, self.n3)));
    }

    fn draw_aa(&self, extents: Vec2, cb: SamplingCallback, param: *mut ()) -> bool {
        let px_inside = 1.0 / c_sqrt(2.0);
        let px_outside = -1.0 / c_sqrt(2.0);
        let bk_size = 8.0f32;
        let bk_inside = c_sqrt(bk_size * bk_size / 2.0);
        let bk_outside = -c_sqrt(bk_size * bk_size / 2.0);
        let mut minx = c_floor(self.v1.x.min(self.v2.x).min(self.v3.x).max(0.0));
        let mut miny = c_floor(self.v1.y.min(self.v2.y).min(self.v3.y).max(0.0));
        let mut maxx = c_ceil(
            self.v1
                .x
                .max(self.v2.x)
                .max(self.v3.x)
                .min(extents.x - 1.0),
        );
        let mut maxy = c_ceil(
            self.v1
                .y
                .max(self.v2.y)
                .max(self.v3.y)
                .min(extents.y - 1.0),
        );
        minx = c_floor(minx);
        miny = c_floor(miny);
        minx += 0.5;
        miny += 0.5;
        maxx += 0.5;
        maxy += 0.5;
        let c1 = self.n1.x * (-self.v1.x) + self.n1.y * (-self.v1.y);
        let c2 = self.n2.x * (-self.v2.x) + self.n2.y * (-self.v2.y);
        let c3 = self.n3.x * (-self.v3.x) + self.n3.y * (-self.v3.y);
        let mut y0 = miny;
        while y0 <= maxy {
            let mut x0 = minx;
            while x0 <= maxx {
                let xc = x0 + (bk_size - 1.0) / 2.0;
                let yc = y0 + (bk_size - 1.0) / 2.0;
                let a_c = c1 + self.n1.x * xc + self.n1.y * yc;
                let b_c = c2 + self.n2.x * xc + self.n2.y * yc;
                let c_c = c3 + self.n3.x * xc + self.n3.y * yc;
                if a_c <= bk_outside || b_c <= bk_outside || c_c <= bk_outside {
                    x0 += bk_size;
                    continue;
                }
                if a_c >= bk_inside && b_c >= bk_inside && c_c >= bk_inside {
                    let mut y = y0;
                    while y < y0 + bk_size {
                        let mut x = x0;
                        while x < x0 + bk_size {
                            if !cb(param, x as i32, y as i32) {
                                return false;
                            }
                            x += 1.0;
                        }
                        y += 1.0;
                    }
                } else {
                    let mut cy1 = c1 + self.n1.x * x0 + self.n1.y * y0;
                    let mut cy2 = c2 + self.n2.x * x0 + self.n2.y * y0;
                    let mut cy3 = c3 + self.n3.x * x0 + self.n3.y * y0;
                    let mut y = y0;
                    while y < y0 + bk_size {
                        let mut cx1 = cy1;
                        let mut cx2 = cy2;
                        let mut cx3 = cy3;
                        let mut x = x0;
                        while x < x0 + bk_size {
                            if cx1 >= px_inside && cx2 >= px_inside && cx3 >= px_inside {
                                if !cb(param, x as i32, y as i32) {
                                    return false;
                                }
                            } else if cx1 >= px_outside && cx2 >= px_outside && cx3 >= px_outside {
                                let mut ct = ClippedTriangle::new(
                                    self.v1 - Vec2::new(x, y),
                                    self.v2 - Vec2::new(x, y),
                                    self.v3 - Vec2::new(x, y),
                                );
                                ct.clip_aa_box(-0.5, -0.5, 0.5, 0.5);
                                if ct.area > 0.0 && !cb(param, x as i32, y as i32) {
                                    return false;
                                }
                            }
                            cx1 += self.n1.x;
                            cx2 += self.n2.x;
                            cx3 += self.n3.x;
                            x += 1.0;
                        }
                        cy1 += self.n1.y;
                        cy2 += self.n2.y;
                        cy3 += self.n3.y;
                        y += 1.0;
                    }
                }
                x0 += bk_size;
            }
            y0 += bk_size;
        }
        true
    }
}

pub fn draw_triangle(extents: Vec2, v: [Vec2; 3], cb: SamplingCallback, param: *mut ()) -> bool {
    let tri = Triangle::new(v[0], v[1], v[2]);
    if tri.is_valid() {
        return tri.draw_aa(extents, cb, param);
    }
    true
}
