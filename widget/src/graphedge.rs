use render::*;
use serde::*;
use uuid::Uuid;

use crate::graphnodeport::*;

#[derive(Clone, PartialEq)]
pub enum GraphEdgeEvent {
    None,
    DragMove { fe: FingerMoveEvent },
    DragEnd { fe: FingerUpEvent },
    DragOut,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub start: GraphNodePortAddress,
    pub end: GraphNodePortAddress,
    pub id: Uuid,
    #[serde(
        skip_serializing,
        skip_deserializing,
        default = "build_default_animator"
    )]
    pub animator: Animator,
}

#[derive(Clone)]
pub struct TempGraphEdge {
    pub start: GraphNodePortAddress,
    pub end: Vec2,
    pub animator: Animator,
}

impl Default for TempGraphEdge {
    fn default() -> Self {
        return Self {
            start: GraphNodePortAddress::default(),
            end: Vec2::zero(),
            animator: Animator::new(Anim::empty()),
        };
    }
}

fn build_default_animator() -> Animator {
    Animator::new(Anim::empty())
}

impl Style for GraphEdge {
    fn style(cx: &mut Cx) -> Self {
        Self {
            start: GraphNodePortAddress::default(),
            end: GraphNodePortAddress::default(),
            id: Uuid::new_v4(),
            animator: Animator::new(Anim::empty()),
        }
    }
}

impl GraphEdge {
    pub fn draw_graph_edge(
        &mut self,
        cx: &mut Cx,
        start: Vec2,
        end: Vec2,
        bg: &mut Quad,
        connector_bg: &mut Quad,
    ) {
        let lb = Vec2 {
            x: start.x.min(end.x),
            y: start.y.min(end.y),
        };
        let ub = Vec2 {
            x: start.x.max(end.x),
            y: start.y.max(end.y),
        };

        let aabb = Rect {
            x: lb.x - 20.,
            y: lb.y - 20.,
            w: ub.x - lb.x + 40.,
            h: ub.y - lb.y + 40.,
        };

        let inst = connector_bg.draw_quad_abs(cx, aabb);

        inst.push_vec2(
            cx,
            Vec2 {
                x: (start.x - aabb.x).abs() + 10.,
                y: (start.y - aabb.y).abs() + 10.,
            },
        );

        inst.push_vec2(
            cx,
            Vec2 {
                x: (end.x - aabb.x).abs() + 10.,
                y: (end.y - aabb.y).abs() + 10.,
            },
        );

        self.animator.update_area_refs(cx, inst.clone().into_area());
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
            _ => (),
        }
        GraphEdgeEvent::None
    }
}

impl TempGraphEdge {
    pub fn draw_graph_edge(
        &mut self,
        cx: &mut Cx,
        start: Vec2,
        end: Vec2,
        bg: &mut Quad,
        connector_bg: &mut Quad,
    ) {
        let lb = Vec2 {
            x: start.x.min(end.x),
            y: start.y.min(end.y),
        };
        let ub = Vec2 {
            x: start.x.max(end.x),
            y: start.y.max(end.y),
        };

        let aabb = Rect {
            x: lb.x - 20.,
            y: lb.y - 20.,
            w: ub.x - lb.x + 40.,
            h: ub.y - lb.y + 40.,
        };

        let inst = connector_bg.draw_quad_abs(cx, aabb);

        inst.push_vec2(
            cx,
            Vec2 {
                x: (start.x - aabb.x).abs() + 10.,
                y: (start.y - aabb.y).abs() + 10.,
            },
        );

        inst.push_vec2(
            cx,
            Vec2 {
                x: (end.x - aabb.x).abs(),
                y: (end.y - aabb.y).abs(),
            },
        );

        self.animator.update_area_refs(cx, inst.clone().into_area());
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
            _ => (),
        }
        GraphEdgeEvent::None
    }
}
