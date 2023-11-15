use core::num;

use crate::{makepad_draw::*, makepad_widgets::*};

live_design! {
    import makepad_draw::shader::std::*;

    DrawLineSegment = {{DrawLineSegment}} {
        fn pixel(self) -> vec4 
        {
            
            let pixelpos = self.pos * self.rect_size;
            let b = self.line_end;
            let a = self.line_start;
            let p = pixelpos;

            let ba = b-a;
            let pa = p-a;
            let h =clamp( dot(pa,ba)/dot(ba,ba), 0.0, 1.0 );
            let dist= length(pa-h*ba)
         
            let linemult = smoothstep(self.width-1., self.width, dist);

            return vec4(self.color.xyz*abs(smoothstep(-0.1,0.1,sin(h*6.283*8.)))*(1.-linemult),1.0-linemult);
        }
    }

    Ball = {{Ball}} {
        fn pixel(self) -> vec4 {
           
            let pixelpos = self.pos * self.rect_size;
            let sdf = Sdf2d::viewport(pixelpos);
           


            sdf.move_to(0.0,0.0);
            sdf.line_to(self.rect_size.x, 0);
            sdf.line_to(self.rect_size.x, self.rect_size.y);
            sdf.line_to(0, self.rect_size.y);
            sdf.line_to(0, 0);
            sdf.fill(#0001);

            return sdf.result

            //return #ff0
        }
    }

    LineChart = {{LineChart}} {
        width: Fill,
        height: Fill
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawLineSegment {
    #[deref]
    draw_super: DrawQuad,
    #[calc]
    line_start: Vec2,
    #[calc]
    line_end: Vec2,
    #[calc]
    width: f32,
    #[calc]
    color: Vec4,
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct Ball {
    #[deref]
    draw_super: DrawQuad,
}

enum DraggingSide {
    LineStartNOTDragging,
    LineStartIsDragging,
    LineEndIsDragging,
}

#[derive(Live)]
pub struct LineChart {
    #[walk]
    walk: Walk,
    #[live]
    draw_ls: DrawLineSegment,
    #[live]
    draw_ball: Ball,
    #[live]
    draw_text: Label,
    #[rust]
    area: Area,
    #[rust]
    _screen_view: Rect,
    #[rust]
    _data_view: Rect,
    #[live(15.0)]
    line_width: f64,
    #[rust(dvec2(1050., 10.))]
    line_start: DVec2,
    #[rust(dvec2(1000., 440.))]
    line_end: DVec2,
    #[rust(dvec2(1000., 140.))]
    line_dragstart: DVec2,
    #[rust(DraggingSide::LineStartNOTDragging)]
    draggingside: DraggingSide,
    #[rust(false)]
    start_hover: bool,

    #[rust(false)]
    end_hover: bool,
    #[rust(0.0)]
    width_slider_value: f64,
    #[rust(0.0)]
    pixel_slider_value: f64,
}

impl Widget for LineChart {
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem),
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut |cx, action| {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid));
        });
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx)
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, WidgetAction)]
pub enum LineChartAction {
    None,
}

impl LiveHook for LineChart {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, LineChart)
    }

    fn after_new_from_doc(&mut self, _cx: &mut Cx) {}
}

impl LineChart {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        // lets draw a bunch of quads
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);

        self.line_width = self.width_slider_value.powf(2.0) * 160. + 10.5;
        let maxpixels = self.pixel_slider_value * 2000.0 + 10.;

        /*if true{
                let r = Rect{
                    pos: dvec2(0.,0.),
                    size: dvec2(2000.,2000.)
                };

                let mut actualstart =self.line_start;
                let mut actualend =self.line_end;

                self.draw_ls.line_start = (actualstart - r.pos).into_vec2();
                self.draw_ls.line_end = (actualend - r.pos).into_vec2();
                self.draw_ls.width = (self.line_width as f32)*1.3;
                self.draw_ls.color = vec4(0.6,0.6,0.0,0.30);


                self.draw_ls.draw_abs(cx, r);
            }
        */
        if self.start_hover {
            let r = Rect {
                pos: self.line_start - dvec2(30., 30.),
                size: dvec2(60., 60.),
            };
            self.draw_ball.draw_abs(cx, r);
        }

        if self.end_hover {
            let r = Rect {
                pos: self.line_end - dvec2(30., 30.),
                size: dvec2(60., 60.),
            };
            self.draw_ball.draw_abs(cx, r);
        }

        let hw = self.line_width / 2.;
        self.draw_ls.width = hw as f32;
        self.draw_ls.color = vec4(1., 1., 0.2, 1.0);

        let linerect = self.line_end - self.line_start;
        if (self.line_start.y - self.line_end.y).abs().floor() == 0.0
            || (self.line_start.x - self.line_end.x).abs().floor() == 0.0
        {
            let r = Rect {
                pos: dvec2(
                    min(self.line_start.x, self.line_end.x) - hw,
                    min(self.line_start.y, self.line_end.y) - hw,
                ),
                size: dvec2(
                    linerect.x.abs() + self.line_width,
                    linerect.y.abs() + self.line_width,
                ),
            };

            self.draw_ls.line_start = (self.line_start - r.pos).into_vec2();
            self.draw_ls.line_end = (self.line_end - r.pos).into_vec2();

            self.draw_ls.draw_abs(cx, r);

            return;
        }

        if linerect.x.abs() > linerect.y.abs()
        // more horizontal than vertical
        {
            let mut actualstart = self.line_start;
            let mut actualend = self.line_end;

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

            //println!("{} {:.1} {:.1} {:.1} {:.1}", angle, clocktang.x, clocktang.y, self.line_start + clocktan * hw, self.line_start);

            let rectstart = Rect {
                pos: actualstart - dvec2(hw, hw),
                size: dvec2(hw - backoffset, self.line_width),
            };
            let rectend = Rect {
                pos: actualend - dvec2(-backoffset, hw),
                size: dvec2(hw - backoffset, self.line_width),
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
            let blockheight = self.line_width / angle.cos() + step.y.abs();

            self.draw_ls.color = vec4(0.9, 0.9, 0.0, 1.0);
            self.draw_ls.width = hw as f32;
            let segmentstart = dvec2(rectstart.pos.x + rectstart.size.x, rectstart.pos.y + adjust);

            for i in 0..(numblocks as i32) as i32 {
                let mut r = Rect {
                    pos: segmentstart + step * (i as f64),
                    size: dvec2(blockwidth, blockheight),
                };
                r.clip_y_between(miny, maxy);

                self.draw_ls.line_start = (actualstart - r.pos).into_vec2();
                self.draw_ls.line_end = (actualend - r.pos).into_vec2();

                self.draw_ls.draw_abs(cx, r);
            }

            self.draw_ls.line_start = (actualstart - rectstart.pos).into_vec2();
            self.draw_ls.line_end = (actualend - rectstart.pos).into_vec2();

            self.draw_ls.draw_abs(cx, rectstart);

            self.draw_ls.line_start = (actualstart - rectend.pos).into_vec2();
            self.draw_ls.line_end = (actualend - rectend.pos).into_vec2();

            self.draw_ls.draw_abs(cx, rectend);


        } else {
             let mut actualstart = self.line_start;
            let mut actualend = self.line_end;

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
                size: dvec2(self.line_width, hw - backoffset),
            };
            let rectend = Rect {
                pos: actualend - dvec2(hw, -backoffset),
                size: dvec2(self.line_width, hw - backoffset),
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
            let blockwidth = self.line_width / angle.cos() + step.x.abs();

            
            self.draw_ls.color = vec4(0.9, 0.9, 0.0, 1.0);
            self.draw_ls.width = hw as f32;
            let segmentstart = dvec2(rectstart.pos.x + adjust, rectstart.pos.y + rectstart.size.y);


            for i in 0..(numblocks as i32) as i32 {
                let mut r = Rect {
                    pos: segmentstart + step * (i as f64),
                    size: dvec2(blockwidth, blockheight),
                };
                r.clip_x_between(minx, maxx);

                self.draw_ls.line_start = (actualstart - r.pos).into_vec2();
                self.draw_ls.line_end = (actualend - r.pos).into_vec2();

                self.draw_ls.draw_abs(cx, r);
            }

            self.draw_ls.line_start = (actualstart - rectstart.pos).into_vec2();
            self.draw_ls.line_end = (actualend - rectstart.pos).into_vec2();

            self.draw_ls.draw_abs(cx, rectstart);

            self.draw_ls.line_start = (actualstart - rectend.pos).into_vec2();
            self.draw_ls.line_end = (actualend - rectend.pos).into_vec2();

            self.draw_ls.draw_abs(cx, rectend);

            
        }
    }

    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, LineChartAction),
    ) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) => {
                let l1 = (fe.abs - self.line_start).lengthsquared();
                let l2 = (fe.abs - self.line_end).lengthsquared();
                if l2 < l1 {
                    //   if (l2<250.)
                    //  {
                    self.draggingside = DraggingSide::LineEndIsDragging;
                    self.line_dragstart = self.line_end;
                    // }
                } else {
                    // if (l1<250.)
                    //  {
                    self.draggingside = DraggingSide::LineStartIsDragging;
                    self.line_dragstart = self.line_start;
                    // }
                }
            }
            Hit::FingerUp(_fe) => {
                self.draggingside = DraggingSide::LineStartNOTDragging;
            }
            Hit::FingerMove(fe) => {
                let rel = fe.abs - fe.abs_start;
                //  log!("{:?}", rel);
                let l1 = (fe.abs - self.line_start).lengthsquared();
                let l2 = (fe.abs - self.line_end).lengthsquared();

                if l1 < 250. {
                    self.start_hover = true;
                } else {
                    self.start_hover = false;
                }
                if l2 < 250. {
                    self.end_hover = true;
                } else {
                    self.end_hover = false;
                }

                if let DraggingSide::LineStartIsDragging = self.draggingside {
                    self.line_start = self.line_dragstart + rel;
                }
                if let DraggingSide::LineEndIsDragging = self.draggingside {
                    self.line_end = self.line_dragstart + rel;
                }
                self.area.redraw(cx);
            }
            _ => (),
        }
    }
}

// ImGUI convenience API for Piano
#[derive(Clone, PartialEq, WidgetRef)]
pub struct LineChartRef(WidgetRef);

impl LineChartRef {
    pub fn set_width_slider_value(&self, _cx: &mut Cx, val: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.width_slider_value = val;
        }
    }

    pub fn set_pixel_slider_value(&self, _cx: &mut Cx, val: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.pixel_slider_value = val;
        }
    }
}
