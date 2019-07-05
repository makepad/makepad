use render::*;

#[derive(Clone, PartialEq)]
pub enum GraphEdgeEvent {
    None,
    DragMove { fe: FingerMoveEvent },
    DragEnd { fe: FingerUpEvent },
    DragOut,
}

#[derive(Clone)]
pub struct GraphEdge {
    pub node_bg_layout: Layout,
    pub node_bg: Quad,
    pub connector_bg: Quad,
    pub start: Vec2,
    pub end: Vec2,
    pub animator: Animator,
}

impl Style for GraphEdge {
    fn style(cx: &mut Cx) -> Self {
        Self {
            node_bg: Quad {
                color: color("#F00"),
                shader: cx.add_shader(Self::def_node_bg_shader(), "GraphEdgeNode.node_bg"),
                ..Quad::style(cx)
            },
            node_bg_layout: Layout {
                abs_origin: Some(Vec2 { x: 100.0, y: 200.0 }),
                width: Bounds::Fix(200.0),
                height: Bounds::Fix(100.0),
                ..Default::default()
            },

            connector_bg: Quad {
                color: color("#0F0"),
                shader: cx.add_shader(Self::def_connector_bg_shader(), "GraphEdgeConnector.node_bg"),
                ..Quad::style(cx)
            },

            start: Vec2{x: 0.0, y: 0.0},
            end: Vec2{x: 100.0, y: 100.0},
            animator: Animator::new(Anim::empty()),
        }
    }
}

impl GraphEdge {
    pub fn def_node_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_circle(0. + w/2.0, 0. + w/2.0, w / 2.0 - 2.);
                return df_fill(color);
            }
        }))
    }
    pub fn def_connector_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            let start: vec2<Instance>;
            let end: vec2<Instance>;

            fn pixel() -> vec4 { 
                df_viewport(pos * vec2(w, h));

                df_move_to(start.x, start.y);
                df_line_to(end.x, end.y);
                df_stroke(color, 2.);

                df_circle(end.x, end.y, 6.);
                df_fill(color("#00F"));

                df_circle(10., 10., 6.);
                return df_fill(color("#F00"));
            }
        }))
    }
    pub fn draw_graph_edge(&mut self, cx: &mut Cx) {

        let layout = Layout {
            abs_origin: Some(Vec2 {
                x: self.start.x - 5.0,
                y: self.start.y - 5.0,
            }),
            abs_size: Some(Vec2 {
                x: (self.end.x - self.start.x).abs() + 10.0,
                y: (self.end.y - self.start.y).abs() + 10.0,
            }),
            ..Default::default()
        };

        let inst = self.connector_bg.draw_quad(cx, Rect {
            x: self.start.x - 20.0,
            y: self.start.y - 20.0,
            w: (self.end.x - self.start.x).abs() + 20.0,
            h: (self.end.y - self.start.y).abs() + 20.0,
        });

        let end = Vec2{
            x: (self.end.x - self.start.x),
            y: (self.end.y - self.start.y),
        };

        inst.push_vec2(cx, Vec2{x:10.0, y:10.0});
        inst.push_vec2(cx, Vec2{
            x: end.x + 10.0,
            y: end.y + 10.0,
        });

        // let inst = self.node_bg.begin_quad(cx, &layout);        

  

        // if (end.x.abs() > end.y.abs()) {
        //     // horizontal diagonal
        //     /*
        //         O
        //          \
        //            ------------
        //                         \
        //                           ---O
        //     */
        //     let half = Vec2{x: end.y / 2.0, y: end.y / 2.0 };
        //     let area = self.connector_bg.draw_quad(cx, Rect {
        //         x: 5.0,//self.start.x - 10.0,
        //         y: 5.0,//self.start.y - 10.0,
        //         w: half.x + 20.,
        //         h: half.y + 20.,
        //     });
        //     area.push_vec2(cx, Vec2{x:10.0, y:10.0});
        //     area.push_vec2(cx, half);


        //     let next = Vec2{ x: end.x - half.x, y: end.y - half.y };
        //     let area = self.connector_bg.draw_quad(cx, Rect {
        //         x: half.x + 10.,
        //         y: half.y + 10.,
        //         w: next.x + 20.,
        //         h: next.y + 20.,
        //     });
        //     area.push_vec2(cx, Vec2{x:2.0, y:2.0});
        //     area.push_vec2(cx, next);


        // } else if (end.y.abs() > end.x.abs()) {
        //     // vertical diagonal

        // } else {
        //     // straight line in any direction
        //     // let area = self.connector_bg.draw_quad_walk(
        //     //     cx,
        //     //     Bounds::Fix(end.x + 5.0),
        //     //     Bounds::Fix(end.y + 5.0),
        //     //     Margin::default()
        //     // );
        //     // area.push_vec2(cx, Vec2{x:10.0, y:10.0});
        //     // area.push_vec2(cx, end);
        // }

        // QUESTION: wow, missing this crucial line causes so much breakage
        // if effectively causes the event.hits method to fail which makes
        // fingerdown to appear broken as well as the fingermove start positions
        // how can this be made less of a footgun?
        self.animator.update_area_refs(cx, inst.clone().into_area());
        // self.node_bg.end_quad(cx, &inst);
    }

    pub fn handle_graph_edge(&mut self, cx: &mut Cx, event: &mut Event) -> GraphEdgeEvent {
        match event.hits(cx, self.animator.area, HitOpt::default()) {
            Event::Animate(ae) => {
                self.animator
                    .write_area(cx, self.animator.area, "bg.", ae.time);
            }
            Event::FingerUp(fe) => {
                return GraphEdgeEvent::DragEnd { fe: fe.clone() };
            }
            Event::FingerMove(fe) => {
                self.node_bg_layout.abs_origin = Some(Vec2 {
                    x: fe.abs.x - fe.rel_start.x,
                    y: fe.abs.y - fe.rel_start.y,
                });

                return GraphEdgeEvent::DragMove { fe: fe.clone() };
            }
            _ => (),
        }
        GraphEdgeEvent::None
    }
}
