use {
    crate::{
        makepad_platform::*,
        //draw_list_2d::ManyInstances,
        //geometry::GeometryQuad2D,
        cx_2d::Cx2d,
        //turtle::{Walk, Layout},
        DrawQuad
    },
};

live_design! {
    import makepad_draw::shader::std::*;
    DrawLine= {{DrawLine}} {
       
        fn stroke(self, side:float, progress: float) -> vec4{
            return self.color;
        }

        fn pixel(self) -> vec4 {
            let p = self.pos * self.rect_size;
            let b = self.line_end;
            let a = self.line_start;
            
            let ba = b-a;
            let pa = p-a;
            let h = clamp( dot(pa,ba)/dot(ba,ba), 0.0, 1.0 );
            let dist= length(pa-h*ba)
            
            let linemult = smoothstep(self.half_line_width-1., self.half_line_width, dist);
            let C = self.stroke(dist, h);
            return vec4(C.xyz*(1.-linemult),(1.0-linemult)*C.a);
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawLine {
    #[deref] pub draw_super: DrawQuad,
    #[calc]  pub line_start: Vec2,
    #[calc]  pub line_end: Vec2,
    #[calc]  pub half_line_width: f32,
    #[calc]  pub color: Vec4,    
}

impl DrawLine
{
    pub fn draw_line_abs(&mut self,  cx: &mut Cx2d, line_start: DVec2, line_end: DVec2, color: Vec4, line_width: f64 )
    {
        let maxpixels = 300. as f64;
        
        let hw = line_width / 2.;

        self.half_line_width = hw as f32;
        self.color = color;


        let linerect = line_end - line_start;
        if (line_start.y - line_end.y).abs().floor() == 0.0
            || (line_start.x - line_end.x).abs().floor() == 0.0
        {
            let r = Rect {
                pos: dvec2(
                    min(line_start.x, line_end.x) - hw,
                    min(line_start.y, line_end.y) - hw,
                ),
                size: dvec2(
                    linerect.x.abs() + line_width,
                    linerect.y.abs() + line_width,
                ),
            };

            self.line_start = (line_start - r.pos).into_vec2();
            self.line_end = (line_end - r.pos).into_vec2();

            self.draw_abs(cx, r);

            return;
        }

        if linerect.x.abs() > linerect.y.abs()
        // more horizontal than vertical
        {
            let mut actualstart = line_start;
            let mut actualend = line_end;

            if actualend.x < actualstart.x {
                std::mem::swap(&mut actualstart, &mut actualend);
            }

            let delta = actualend - actualstart;
            let normalizedelta = delta.normalize();
            let xnormalizedelta = delta.normalize_to_x();
            let normalizedarea = (xnormalizedelta.x * xnormalizedelta.y).abs();
            let scaledup = (maxpixels / normalizedarea).sqrt();

            let angle = delta.angle_in_radians();
            let tanangle = angle.tan();

            let clocktang = normalizedelta.clockwise_tangent();

            let circlepoint = clocktang * hw;
            let overside = hw - circlepoint.y;
            let aanliggend = overside / tanangle;
            let backoffset = circlepoint.x.abs() - aanliggend.abs();


            let rectstart = Rect {
                pos: actualstart - dvec2(hw, hw),
                size: dvec2(hw - backoffset, line_width),
            };

            let rectend = Rect {
                pos: actualend - dvec2(-backoffset, hw),
                size: dvec2(hw - backoffset, line_width),
            };
            
            let miny = min(rectstart.pos.y, rectend.pos.y);
            let maxy = max(
                rectend.pos.y + rectend.size.y,
                rectstart.pos.y + rectstart.size.y,
            );

            let innerwidth = rectend.pos.x - (rectstart.pos.x + rectstart.size.x);
            let numblocks = (innerwidth / scaledup).ceil();
            let blockwidth = innerwidth / (numblocks as f64);

            let step = dvec2(blockwidth, xnormalizedelta.y * blockwidth);
            let mut adjust = -backoffset * 2. * xnormalizedelta.y;
            if step.y < 0. {
                adjust = step.y;
            }
            let blockheight = line_width / angle.cos() + step.y.abs();

            let segmentstart = dvec2(rectstart.pos.x + rectstart.size.x, rectstart.pos.y + adjust);

            for i in 0..(numblocks as i32) as i32 {
                let mut r = Rect {
                    pos: segmentstart + step * (i as f64),
                    size: dvec2(blockwidth, blockheight),
                };
                r.clip_y_between(miny, maxy);

                self.line_start = (actualstart - r.pos).into_vec2();
                self.line_end = (actualend - r.pos).into_vec2();

                self.draw_abs(cx, r);
            }

            self.line_start = (actualstart - rectstart.pos).into_vec2();
            self.line_end = (actualend - rectstart.pos).into_vec2();

            self.draw_abs(cx, rectstart);

            self.line_start = (actualstart - rectend.pos).into_vec2();
            self.line_end = (actualend - rectend.pos).into_vec2();

            self.draw_abs(cx, rectend);


        } else {
             let mut actualstart = line_start;
            let mut actualend: DVec2 = line_end;

            if actualend.y < actualstart.y {
                std::mem::swap(&mut actualstart, &mut actualend);
            }
            let delta = actualend - actualstart;
            let normalizedelta = delta.normalize();
            let ynormalizedelta = delta.normalize_to_y();
            let normalizedarea = (ynormalizedelta.x * ynormalizedelta.y).abs();
            let scaledup = (maxpixels / normalizedarea).sqrt();
            let angle =  delta.angle_in_radians() - std::f64::consts::PI/2.;
            let tanangle = angle.tan();  
            let circlepoint = normalizedelta * hw;
            let overside = hw - circlepoint.y;
            let aanliggend = overside / tanangle;
            let backoffset = circlepoint.x.abs() - aanliggend.abs();

            let rectstart = Rect {
                pos: actualstart - dvec2(hw, hw),
                size: dvec2(line_width, hw - backoffset),
            };
            let rectend = Rect {
                pos: actualend - dvec2(hw, -backoffset),
                size: dvec2(line_width, hw - backoffset),
            };
            let minx = min(rectstart.pos.x, rectend.pos.x);
            let maxx = max(
                rectend.pos.x + rectend.size.x,
                rectstart.pos.x + rectstart.size.x,
            );

            let innerheight = rectend.pos.y - (rectstart.pos.y + rectstart.size.y);
            let numblocks = (innerheight / scaledup).ceil();
            let blockheight = innerheight / (numblocks as f64);

            let step = dvec2( ynormalizedelta.x * blockheight, blockheight);
            let mut adjust = -backoffset * 2. * ynormalizedelta.x;
            if step.x < 0. {
                adjust = step.x;
            }
            let blockwidth = line_width / angle.cos() + step.x.abs();

            
          
            let segmentstart = dvec2(rectstart.pos.x + adjust, rectstart.pos.y + rectstart.size.y);


            for i in 0..(numblocks as i32) as i32 {
                let mut r = Rect {
                    pos: segmentstart + step * (i as f64),
                    size: dvec2(blockwidth, blockheight),
                };
                r.clip_x_between(minx, maxx);

                self.line_start = (actualstart - r.pos).into_vec2();
                self.line_end = (actualend - r.pos).into_vec2();

                self.draw_abs(cx, r);
            }

            self.line_start = (actualstart - rectstart.pos).into_vec2();
            self.line_end = (actualend - rectstart.pos).into_vec2();

            self.draw_abs(cx, rectstart);

            self.line_start = (actualstart - rectend.pos).into_vec2();
            self.line_end = (actualend - rectend.pos).into_vec2();

            self.draw_abs(cx, rectend);

            
        }
    }

}