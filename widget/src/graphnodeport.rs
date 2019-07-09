use render::*;
use serde::*;
use uuid::Uuid;

#[derive(Clone, PartialEq)]
pub enum GraphNodePortEvent {
    None,
    Handled,
    DragMove { fe: FingerMoveEvent },
    DragEnd { fe: FingerUpEvent },
    DropHit,
    DropMiss,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PortDirection {
    None,
    Input,
    Output,
}

impl Default for PortDirection {
    fn default() -> Self {
        PortDirection::None
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GraphNodePortAddress {
    pub node: Uuid,
    pub port: Uuid,
    // an extra qualifier to aid in lookup
    #[serde(default = "build_default_port_direction")]
    pub dir: PortDirection,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct GraphNodePort {
    pub id: Uuid,
    pub aabb: Rect,
    #[serde(
        skip_serializing,
        skip_deserializing,
        default = "build_default_animator"
    )]
    pub animator: Animator,
}

fn build_default_port_direction() -> PortDirection {
    PortDirection::Input
}

fn build_default_animator() -> Animator {
    Animator::new(Anim::empty())
}

impl Style for GraphNodePort {
    fn style(_cx: &mut Cx) -> Self {
        Self {
            aabb: Rect::default(),
            id: Uuid::new_v4(),
            animator: Animator::new(Anim::empty()),
        }
    }
}

impl GraphNodePort {
    pub fn draw(&mut self, cx: &mut Cx, bg: &mut Quad, aabb: Rect) {
        // let inst = bg.draw_quad_abs(cx, &self.node_bg_layout);
        self.aabb = aabb.clone();
        let inst = bg.draw_quad_abs(cx, aabb);
        self.animator.update_area_refs(cx, inst.clone().into_area());
        //bg.end_quad(cx, &inst);
    }

    pub fn handle(&mut self, cx: &mut Cx, event: &mut Event) -> GraphNodePortEvent {
        match event.hits(cx, self.animator.area, HitOpt::default()) {
            Event::Animate(ae) => {
                self.animator
                    .write_area(cx, self.animator.area, "bg.", ae.time);
            }
            Event::FingerMove(fe) => {
                return GraphNodePortEvent::DragMove { fe: fe };
            }
            Event::FingerUp(fe) => {
                return GraphNodePortEvent::DragEnd { fe: fe };
            }
            Event::FingerHover(fe) => match fe.hover_state {
                HoverState::In => {
                    return GraphNodePortEvent::DropHit;
                }
                HoverState::Out => {
                    return GraphNodePortEvent::DropMiss;
                }
                _ => (),
            },
            _ => (),
        }
        GraphNodePortEvent::None
    }
}
