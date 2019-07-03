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
    pub animator: Animator,
}

impl Style for GraphEdge {
    fn style(cx: &mut Cx) -> Self {
        Self {
            node_bg: Quad {
                color: color("#F00"),
                ..Quad::style(cx)
            },
            node_bg_layout: Layout {
                abs_origin: Some(Vec2 { x: 100.0, y: 200.0 }),
                width: Bounds::Fix(200.0),
                height: Bounds::Fix(100.0),
                ..Default::default()
            },
            animator: Animator::new(Anim::empty()),
        }
    }
}

impl GraphEdge {
    pub fn draw_graph_node(&mut self, cx: &mut Cx) {
        let inst = self.node_bg.begin_quad(cx, &self.node_bg_layout);
        // QUESTION: wow, missing this crucial line causes so much breakage
        // if effectively causes the event.hits method to fail which makes
        // fingerdown to appear broken as well as the fingermove start positions
        self.animator.update_area_refs(cx, inst.clone().into_area());
        self.node_bg.end_quad(cx, &inst);
    }

    pub fn handle_graph_node(&mut self, cx: &mut Cx, event: &mut Event) -> GraphEdgeEvent {
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
